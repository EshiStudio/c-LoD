//! The c-LoD engine: a worker thread that polls players, plans LOD deltas and
//! feeds bounded op batches to the server main thread.
//!
//! Data flow:
//! ```text
//! worker ──JvmOp──▶ main-thread drain (Bukkit / TicketStorage)
//!    ▲                       │
//!    └──── JvmResult ────────┘
//! ```
//! The worker owns all planning state (chunk cache, active registry, pin
//! set). JVM failures degrade gracefully: missing entities respawn, failed
//! pins retry next cycle. No panics escape the worker (catch_unwind wrap).

use crate::config::Config;
use crate::plan::{self, ActiveEntity, CellVisual, PinChunk};
use crate::quadtree::{self, CellKey, Levels};
use crate::state::msg::{JvmOp, JvmResult, PlayerSnapshot};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Counters surfaced by the periodic log line (and CI greps).
#[derive(Default)]
pub struct Metrics {
    pub cycles: AtomicU64,
    pub spawns: AtomicU64,
    pub updates: AtomicU64,
    pub despawns: AtomicU64,
    pub pins: AtomicU64,
    pub snapshots: AtomicU64,
    pub cells_active: AtomicU64,
    pub players_seen: AtomicU64,
}

struct Channels {
    ops_rx: Mutex<Receiver<JvmOp>>,
    results_tx: Sender<JvmResult>,
}

static CHANNELS: OnceLock<Channels> = OnceLock::new();

/// Main-thread kick callback (registered by lib.rs — keeps engine.rs JVM-free).
static KICK: Mutex<Option<Box<dyn Fn() + Send + Sync>>> = Mutex::new(None);

pub fn set_kick(f: Box<dyn Fn() + Send + Sync>) {
    *KICK.lock().unwrap() = Some(f);
}

fn kick() {
    if let Some(f) = KICK.lock().unwrap().as_ref() {
        f();
    }
}

/// Create the engine channels and start the worker thread. Idempotent.
pub fn start(cfg: Config) -> bool {
    let (ops_tx, ops_rx) = std::sync::mpsc::channel::<JvmOp>();
    let (results_tx, results_rx) = std::sync::mpsc::channel::<JvmResult>();
    let metrics = Arc::new(Metrics::default());
    let m2 = metrics.clone();
    let worker_tx = ops_tx.clone();
    let started = std::thread::Builder::new()
        .name("c-LoD-worker".into())
        .spawn(move || {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                worker_loop(worker_tx, results_rx, cfg, m2)
            }));
            eprintln!("[c-LoD] worker exited");
        })
        .is_ok();
    let _ = CHANNELS.set(Channels {
        ops_rx: Mutex::new(ops_rx),
        results_tx,
    });
    let _ = metrics;
    started
}

/// Take ops queued by the worker (main-thread drain side).
pub fn take_ops() -> Vec<JvmOp> {
    match CHANNELS.get() {
        Some(c) => {
            let rx = c.ops_rx.lock().unwrap();
            let mut out = Vec::new();
            while let Ok(op) = rx.try_recv() {
                out.push(op);
            }
            out
        }
        None => Vec::new(),
    }
}

/// Send a result back to the worker (main-thread drain side).
pub fn send_result(r: JvmResult) -> bool {
    match CHANNELS.get() {
        Some(c) => c.results_tx.send(r).is_ok(),
        None => false,
    }
}

// ---------------------------------------------------------------------------
// Worker
// ---------------------------------------------------------------------------

struct WorkerState {
    chunks: HashMap<(String, i32, i32), crate::snapshot::Column>,
    active: HashMap<CellKey, ActiveEntity>,
    pending: HashMap<CellKey, CellVisual>,
    pinned: HashSet<PinChunk>,
    players: Vec<PlayerSnapshot>,
    view_distance: u32,
    booted: bool,
    last_log: Instant,
}

fn worker_loop(
    ops_tx: Sender<JvmOp>,
    results_rx: Receiver<JvmResult>,
    cfg: Config,
    metrics: Arc<Metrics>,
) {
    let mut st = WorkerState {
        chunks: HashMap::new(),
        active: HashMap::new(),
        pending: HashMap::new(),
        pinned: HashSet::new(),
        players: Vec::new(),
        view_distance: cfg.view_distance.max(2),
        booted: false,
        last_log: Instant::now(),
    };
    let mut levels: Option<Levels> = None;
    let interval = Duration::from_millis(cfg.update_interval_ms.max(50));
    let log_period = Duration::from_millis(
        (cfg.log_interval_cycles as u64).max(1) * cfg.update_interval_ms.max(50),
    );

    loop {
        // 1. drain JVM results
        while let Ok(r) = results_rx.try_recv() {
            apply(&mut st, r, &metrics);
        }

        if st.booted {
            if crate::jvm::bukkit::api().is_none() {
                // bootstrap failed — nothing safe to plan; keep polling
                std::thread::sleep(interval);
                continue;
            }
            if levels.is_none() {
                let vd = if cfg.view_distance == 0 {
                    st.view_distance.max(2)
                } else {
                    cfg.view_distance.max(2)
                };
                levels = Some(Levels::build(vd, cfg.growth, cfg.max_distance / 16));
                eprintln!(
                    "[c-LoD] levels: {:?} (max {} chunks)",
                    levels.as_ref().unwrap().starts,
                    cfg.max_distance / 16
                );
            }
            run_cycle(&mut st, levels.as_ref().unwrap(), &cfg, &metrics, &ops_tx);
            // hand the queued ops to the main thread
            kick();
        }

        // 2. periodic status line (CI greps this)
        if st.last_log.elapsed() >= log_period {
            st.last_log = Instant::now();
            eprintln!(
                "[c-LoD] cells={} pending={} pinned={} chunk_cache={} players={} spawns={} updates={} despawns={} pins={} snapshots={} cycles={}",
                st.active.len(),
                st.pending.len(),
                st.pinned.len(),
                st.chunks.len(),
                st.players.len(),
                metrics.spawns.load(Ordering::Relaxed),
                metrics.updates.load(Ordering::Relaxed),
                metrics.despawns.load(Ordering::Relaxed),
                metrics.pins.load(Ordering::Relaxed),
                metrics.snapshots.load(Ordering::Relaxed),
                metrics.cycles.load(Ordering::Relaxed),
            );
        }

        std::thread::sleep(interval);
    }
}

fn apply(st: &mut WorkerState, r: JvmResult, metrics: &Arc<Metrics>) {
    match r {
        JvmResult::Players(players) => {
            metrics
                .players_seen
                .store(players.len() as u64, Ordering::Relaxed);
            st.players = players;
        }
        JvmResult::Snapshot(Some(d)) => {
            let col = crate::snapshot::Column::from_samples(&d.samples, d.min_world_y);
            st.chunks.insert((d.world, d.cx, d.cz), col);
        }
        JvmResult::Snapshot(None) => {}
        JvmResult::Dead(uuid) => {
            if !uuid.is_empty() {
                st.active.retain(|_, e| e.uuid != uuid);
            }
        }
        JvmResult::Spawned { key, uuid } => {
            if let Some(visual) = st.pending.remove(&key) {
                st.active.insert(key, ActiveEntity { uuid, visual });
            }
        }
        JvmResult::PinAck {
            pin,
            ok,
            world,
            cx,
            cz,
        } => {
            let p = PinChunk { world, cx, cz };
            if pin {
                if !ok {
                    st.pinned.remove(&p); // retry next cycle
                }
            } else if !ok {
                st.pinned.insert(p); // keep trying to release
            }
        }
        JvmResult::Boot {
            view_distance,
            note,
        } => {
            st.view_distance = view_distance.max(2);
            st.booted = true;
            eprintln!(
                "[c-LoD] engine booted: view_distance={} {}",
                view_distance, note
            );
        }
    }
}

/// One planning + op-emission cycle.
fn run_cycle(
    st: &mut WorkerState,
    levels: &Levels,
    cfg: &Config,
    metrics: &Arc<Metrics>,
    ops_tx: &Sender<JvmOp>,
) {
    metrics.cycles.fetch_add(1, Ordering::Relaxed);

    if st.players.is_empty() {
        // nobody around: release everything
        for (_, e) in st.active.drain() {
            if !e.uuid.is_empty() {
                let _ = ops_tx.send(JvmOp::Despawn { uuid: e.uuid });
            }
        }
        st.pending.clear();
        for p in st.pinned.drain() {
            let _ = ops_tx.send(JvmOp::Unpin {
                world: p.world,
                cx: p.cx,
                cz: p.cz,
            });
        }
        let _ = ops_tx.send(JvmOp::ReadPlayers);
        return;
    }

    // desired cells per world (merged across players), prior = active+pending
    let mut prior: HashSet<CellKey> = st.active.keys().cloned().collect();
    prior.extend(st.pending.keys().cloned());
    let mut desired: HashMap<CellKey, Option<CellVisual>> = HashMap::new();
    let mut per_world: HashMap<String, Vec<quadtree::PlayerPos>> = HashMap::new();
    for p in &st.players {
        per_world
            .entry(p.world.clone())
            .or_default()
            .push(quadtree::PlayerPos { cx: p.cx, cz: p.cz });
    }
    for (world, players) in &per_world {
        for p in players {
            for c in quadtree::desired_cells(
                world,
                *p,
                levels,
                cfg.hysteresis,
                cfg.max_distance / 16,
                &prior,
            ) {
                desired.insert(c, None);
            }
        }
    }

    // visuals from the chunk cache (None -> planner requests LOD0 snapshots)
    let keys: Vec<CellKey> = desired.keys().cloned().collect();
    for key in keys {
        let vis = plan::aggregate_visual(&key, |x, z| {
            st.chunks.get(&(key.world.clone(), x, z)).copied()
        });
        if vis.is_some() {
            desired.insert(key, vis);
        }
    }

    // global cell cap: keep the nearest cells
    let centers: Vec<quadtree::PlayerPos> = st
        .players
        .iter()
        .map(|p| quadtree::PlayerPos { cx: p.cx, cz: p.cz })
        .collect();
    if desired.len() as u32 > cfg.max_cells_total {
        let mut ranked: Vec<(&CellKey, i64)> = desired
            .keys()
            .map(|k| (k, nearest_dist(k, &centers)))
            .collect();
        ranked.sort_by_key(|(_, d)| *d);
        let keep: HashSet<CellKey> = ranked
            .into_iter()
            .take(cfg.max_cells_total as usize)
            .map(|(k, _)| k.clone())
            .collect();
        desired.retain(|k, _| keep.contains(k));
    }

    let plan = plan::plan_cycle(&desired, &st.active, &st.pinned, &centers, cfg);

    // emit ops + mirror into worker state
    for (key, visual) in &plan.spawns {
        st.pending.insert(key.clone(), visual.clone());
        let _ = ops_tx.send(JvmOp::Spawn {
            key: key.clone(),
            visual: visual.clone(),
        });
        metrics.spawns.fetch_add(1, Ordering::Relaxed);
    }
    for (key, uuid, visual) in &plan.updates {
        if !uuid.is_empty() {
            let _ = ops_tx.send(JvmOp::Update {
                key: key.clone(),
                uuid: uuid.clone(),
                visual: visual.clone(),
            });
            metrics.updates.fetch_add(1, Ordering::Relaxed);
        }
        if let Some(e) = st.active.get_mut(key) {
            e.visual = visual.clone();
        }
    }
    for (key, uuid) in &plan.despawns {
        if !uuid.is_empty() {
            let _ = ops_tx.send(JvmOp::Despawn { uuid: uuid.clone() });
            metrics.despawns.fetch_add(1, Ordering::Relaxed);
        }
        st.active.remove(key);
    }
    for p in &plan.pins {
        let _ = ops_tx.send(JvmOp::Pin {
            world: p.world.clone(),
            cx: p.cx,
            cz: p.cz,
        });
        metrics.pins.fetch_add(1, Ordering::Relaxed);
    }
    for p in &plan.unpins {
        let _ = ops_tx.send(JvmOp::Unpin {
            world: p.world.clone(),
            cx: p.cx,
            cz: p.cz,
        });
    }
    for (key, cx, cz) in &plan.need_snapshots {
        let _ = ops_tx.send(JvmOp::ReadSnapshot {
            world: key.world.clone(),
            cx: *cx,
            cz: *cz,
        });
        metrics.snapshots.fetch_add(1, Ordering::Relaxed);
    }
    let _ = ops_tx.send(JvmOp::ReadPlayers);
    metrics
        .cells_active
        .store(desired.len() as u64, Ordering::Relaxed);
}

fn nearest_dist(key: &CellKey, centers: &[quadtree::PlayerPos]) -> i64 {
    let mut best = i64::MAX;
    for p in centers {
        let d = quadtree::cell_center_distance(p.cx, p.cz, key.cx, key.cz, key.size());
        best = best.min((d * 1000.0) as i64);
    }
    best
}
