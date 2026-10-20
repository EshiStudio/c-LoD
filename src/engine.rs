//! The c-LoD engine: a state machine driven from the server main thread.
//!
//! STATIC-PACK lifecycle (v3) — the resource pack is built EXACTLY ONCE per
//! session and never rebuilt, never re-pushed with new bytes:
//!
//!   Boot → (restore `c-LoD-atlas.bin` if present and matching)
//!        → Scanning (budgeted JNI heightmap sweeps, main thread)
//!        → Finalize (sweep complete OR `scan_deadline_secs` reached):
//!            1. persist the atlas snapshot to disk (if `atlas_cache`)
//!            2. build the pack from the frozen snapshot
//!            3. swap it into the embedded HTTP server — done, forever
//!        → Finalized: zero pack work; the ONLY dynamic part left is
//!          per-player `/posteffect` tier switching, which touches no bytes.
//!
//! Auto-updating LOD without pack rebuilds:
//!   - Client-side: the `clod:lod_terrain` chain raymarches the world-anchored
//!     heightfield atlas from the live camera every frame — walking, flying or
//!     turning re-projects the LOD surface automatically.
//!   - Server-side: every tick each player is placed in a tier and chains are
//!     added/removed accordingly:
//!       * Tier::Off   — other world / no data      → no chains
//!       * Tier::Veil  — atlas world, outside window → `clod:atmosphere` only
//!       * Tier::Full  — inside the atlas window     → terrain + atmosphere
//!   - Across restarts: the restored snapshot reproduces byte-identical pack
//!     (same SHA-1), so vanilla clients never re-download.

use crate::atlas::Atlas;
use crate::config::Config;
use crate::http::PackServer;
use crate::jni::{self, PlayerInfo};
use crate::packgen::{self, Pack};
use jvmti_bindings::prelude::JniEnv;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, RwLock};

pub const PHASE_BOOT: u8 = 0;
pub const PHASE_SCANNING: u8 = 1;
pub const PHASE_PACKING: u8 = 2;
pub const PHASE_FINALIZED: u8 = 3;

pub struct State {
    pub cfg: Config,
    pub atlas: Arc<Atlas>,
    pub server: Arc<PackServer>,
    pub pack: RwLock<Option<Arc<Pack>>>,
    /// Static-pack phase machine (see PHASE_*).
    pub phase: AtomicU8,
    /// True when the atlas came from the disk cache (=> finalize immediately
    /// after the world resolves; bytes must match the previous session).
    pub restored: bool,
    /// Set after the one-time chunk warm-up around the atlas center.
    pub warmed: AtomicU8,
    /// When the atlas world was first resolved (scan deadline anchor).
    pub world_seen_at: Mutex<Option<std::time::Instant>>,
    pub scan_seq: AtomicU64,
    pub world_obj: Mutex<usize>, // jobject handle as usize; main-thread only
    pub world_name: Mutex<String>,
    pub keys: Mutex<Option<Keys>>,
    pub per_player: Mutex<HashMap<String, PlayerState>>,
    pub pack_uuid: Mutex<usize>,
}

#[derive(Clone)]
pub struct Keys {
    pub terrain: usize, // jobject global refs (main-thread only)
    pub atmosphere: usize,
}

#[derive(Default, Clone)]
pub struct PlayerState {
    /// SHA-1 of the pack last pushed to this player. The pack is static, so
    /// after the first successful push this matches forever and we never
    /// push again — no reloads, no flicker, no re-downloads.
    pub pushed_sha: String,
    pub terrain_on: bool,
    pub atmosphere_on: bool,
}

/// Per-player LOD tier, re-evaluated every tick. Switching tiers is pure
/// `/posteffect` add/remove — the pack is never touched.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tier {
    /// Other world or no coverage data: no chains applied.
    Off,
    /// Atlas world but outside the static window: depth veil only.
    Veil,
    /// Inside the static atlas window: terrain + veil.
    Full,
}

impl State {
    pub fn new(cfg: Config) -> Arc<Self> {
        // Static-pack determinism: restore the frozen snapshot when it
        // matches the configured geometry; otherwise start an empty atlas.
        let (atlas, restored) = if cfg.atlas_cache {
            match Atlas::try_restore(
                std::path::Path::new(&cfg.atlas_path),
                cfg.center_x,
                cfg.center_z,
                cfg.atlas_size,
                cfg.cell_size,
                cfg.sea_level,
            ) {
                Some(a) => (Arc::new(a), true),
                None => (
                    Arc::new(Atlas::new(
                        cfg.center_x,
                        cfg.center_z,
                        cfg.atlas_size,
                        cfg.cell_size,
                        cfg.sea_level,
                    )),
                    false,
                ),
            }
        } else {
            (
                Arc::new(Atlas::new(
                    cfg.center_x,
                    cfg.center_z,
                    cfg.atlas_size,
                    cfg.cell_size,
                    cfg.sea_level,
                )),
                false,
            )
        };
        let server = PackServer::new();
        Arc::new(Self {
            atlas,
            server,
            pack: RwLock::new(None),
            phase: AtomicU8::new(PHASE_BOOT),
            restored,
            warmed: AtomicU8::new(0),
            world_seen_at: Mutex::new(None),
            scan_seq: AtomicU64::new(0),
            world_obj: Mutex::new(0),
            world_name: Mutex::new(String::new()),
            keys: Mutex::new(None),
            per_player: Mutex::new(HashMap::new()),
            pack_uuid: Mutex::new(0),
            cfg,
        })
    }
}

type Env<'a> = &'a JniEnv;

static BRIDGE_OK: AtomicU8 = AtomicU8::new(0); // 0 pending, 1 ok, 2 failed-once

/// One engine tick. MUST run on the main thread.
pub fn tick(state: &Arc<State>, env: Env) {
    ensure_boot(state, env);
    scan_batch(state, env);
    release_forceload(state, env);
    manage_players(state, env);
}

/// One-time: after finalize, release the temporary force-load on the warm
/// core (the snapshot is taken; a static pack needs no live chunks). Runs on
/// the main thread because it touches JNI; CAS makes it exactly-once.
fn release_forceload(state: &Arc<State>, env: Env) {
    if state.warmed.load(Ordering::SeqCst) != 1
        || state.phase.load(Ordering::SeqCst) != PHASE_FINALIZED
    {
        return;
    }
    if state
        .warmed
        .compare_exchange(1, 2, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return;
    }
    let world_handle = *state.world_obj.lock().unwrap();
    if world_handle == 0 {
        return;
    }
    let world = unsafe { as_obj(world_handle) };
    let w = state.cfg.warm_chunks;
    let mut released = 0usize;
    for cx in -w..=w {
        for cz in -w..=w {
            if jni::warm_chunk(env, world, cx, cz, false) {
                released += 1;
            }
        }
    }
    crate::log_info(&format!(
        "c-LoD core force-load released ({released} chunks) — static snapshot retained"
    ));
}

fn ensure_boot(state: &Arc<State>, env: Env) {
    // Resolve JNI ids once the kernel surface is fully loadable. Paper loads
    // the player-surface classes lazily, so warmup retries each tick until
    // the whole surface resolves (see jni::warmup).
    if BRIDGE_OK.load(Ordering::SeqCst) != 1 {
        if jni::warmup(env) {
            BRIDGE_OK.store(1, Ordering::SeqCst);
            crate::log_info("c-LoD JNI bridge resolved: true");
            // Static-pack phase machine: boot complete -> scanning.
            let _ = state.phase.compare_exchange(
                PHASE_BOOT,
                PHASE_SCANNING,
                Ordering::SeqCst,
                Ordering::SeqCst,
            );
            // Effect keys (global refs, main thread).
            let terrain = jni::make_key(env, &state.cfg.effect_terrain)
                .map(|k| unsafe { new_global(env, k) })
                .unwrap_or(0);
            let atmosphere = jni::make_key(env, &state.cfg.effect_atmosphere)
                .map(|k| unsafe { new_global(env, k) })
                .unwrap_or(0);
            *state.keys.lock().unwrap() = Some(Keys {
                terrain,
                atmosphere,
            });
            // Pack UUID global.
            let uuid = jni::parse_uuid(env, &state.cfg.pack_uuid)
                .map(|u| unsafe { new_global(env, u) })
                .unwrap_or(0);
            *state.pack_uuid.lock().unwrap() = uuid;
        } else {
            if BRIDGE_OK.swap(2, Ordering::SeqCst) == 0 {
                crate::log_info(
                    "c-LoD JNI surface incomplete — Paper loads kernel classes lazily; retrying each tick",
                );
            }
            return;
        }
    }

    // Resolve the atlas world once (and re-resolve if lost).
    {
        let mut w = state.world_obj.lock().unwrap();
        if *w == 0 {
            if let Some(world) = jni::find_world(env, &state.cfg.world) {
                let name = world.name.clone();
                let g = unsafe { new_global(env, world.obj) };
                *w = g;
                *state.world_name.lock().unwrap() = name.clone();
                crate::log_info(&format!("c-LoD atlas world: {name}"));
                if state.restored {
                    crate::log_info(&format!(
                        "c-LoD atlas restored from {} ({}% coverage) — pack will reproduce the previous session's SHA-1",
                        state.cfg.atlas_path,
                        state.atlas.scanned_cells() * 100 / state.atlas.total_cells().max(1)
                    ));
                }
                // Anchor the scan deadline at world resolution.
                *state.world_seen_at.lock().unwrap() = Some(std::time::Instant::now());
                // One-time core warm-up: modern vanilla keeps no spawn
                // chunks loaded and the scan never forces generation, so
                // without this the static snapshot could cover nothing.
                if state.cfg.warm_chunks > 0 {
                    let w = state.cfg.warm_chunks;
                    let mut loaded = 0usize;
                    for cx in -w..=w {
                        for cz in -w..=w {
                            if jni::warm_chunk(env, unsafe { as_obj(g) }, cx, cz, true) {
                                loaded += 1;
                            }
                        }
                    }
                    state.warmed.store(1, Ordering::SeqCst);
                    let loaded0 = jni::is_chunk_loaded(env, unsafe { as_obj(g) }, 0, 0);
                    let h0 = jni::probe_height(env, unsafe { as_obj(g) }, 0, 0);
                    crate::log_info(&format!(
                        "c-LoD core force-loaded: {}x{} chunks around atlas center ({} ok) — isChunkLoaded(0,0)={loaded0:?}, height(0,0)={h0:?} — released at finalize",
                        2 * w + 1,
                        2 * w + 1,
                        loaded
                    ));
                }
            }
        }
    }
}

/// # Safety
/// `obj` must be a valid local ref; runs on the main thread only.
unsafe fn new_global(env: Env, obj: jvmti_bindings::jni::jobject) -> usize {
    env.new_global_ref(obj) as usize
}

unsafe fn as_obj(handle: usize) -> jvmti_bindings::jni::jobject {
    handle as jvmti_bindings::jni::jobject
}

fn scan_batch(state: &Arc<State>, env: Env) {
    // Static atlas: after finalize the snapshot is frozen — never scan again.
    if state.phase.load(Ordering::SeqCst) != PHASE_SCANNING {
        return;
    }
    let world_handle = *state.world_obj.lock().unwrap();
    if world_handle == 0 {
        return;
    }
    let world = unsafe { as_obj(world_handle) };
    let seq = state.scan_seq.fetch_add(1, Ordering::Relaxed);
    let budget = state.cfg.scan_budget;
    // Advance the sweep by `budget` cells per tick (scan_task wraps the
    // start index over the whole atlas): passing the bare tick number made
    // every tick rescan cells 0..budget+bumped-by-one, leaving most of the
    // atlas (and the warm core) unreached until the deadline fired.
    let tasks = state.atlas.scan_task(seq * budget as u64, budget);
    for (ix, iz) in tasks.iter().copied() {
        let x = state.atlas.cell_world_x(ix);
        let z = state.atlas.cell_world_z(iz);
        if let Some((y, liquid)) = jni::sample_height(env, world, x, z) {
            state.atlas.set(ix, iz, (y.clamp(-64, 1023)) as i16, liquid);
        }
    }
    // Rate-limited scan diagnostics (one line per 25 ticks).
    if seq.is_multiple_of(25) {
        use std::sync::atomic::Ordering as O;
        crate::log_info(&format!(
            "scan: tick {seq}, atlas v{} ({}/{} cells), ok={} unloaded={} err={}",
            state.atlas.version(),
            state.atlas.scanned_cells(),
            state.atlas.total_cells(),
            jni::SCAN_OK.load(O::Relaxed),
            jni::SCAN_UNLOADED.load(O::Relaxed),
            jni::SCAN_ERR.load(O::Relaxed),
        ));
    }
}

/// Tier for one player from world + static window bounds.
fn tier_for(state: &Arc<State>, p: &PlayerInfo) -> Tier {
    if p.world.is_empty() {
        return Tier::Off;
    }
    let wname = state.world_name.lock().unwrap().clone();
    if !wname.is_empty() && p.world != wname {
        return Tier::Off;
    }
    if state.atlas.cell_index(p.bx, p.bz).is_some() {
        Tier::Full
    } else {
        Tier::Veil
    }
}

fn manage_players(state: &Arc<State>, env: Env) {
    let keys = state.keys.lock().unwrap().clone();
    let Some(keys) = keys else { return };
    if keys.terrain == 0 && keys.atmosphere == 0 {
        return;
    }
    let pack = state.pack.read().unwrap().clone();

    let players = jni::players(env);
    let mut seen: Vec<String> = Vec::new();

    for p in players {
        seen.push(p.uuid.clone());
        let mut st = state
            .per_player
            .lock()
            .unwrap()
            .entry(p.uuid.clone())
            .or_default()
            .clone();

        // 1. Resource pack — pushed exactly once per player per session.
        //    The pack is static; its SHA-1 never changes, so once pushed we
        //    never call setResourcePack again (no client reloads, ever).
        if let Some(pack) = &pack {
            if st.pushed_sha != pack.sha1_hex {
                if let Some(handle) = jni::player_by_uuid(env, &p.uuid) {
                    let pu = *state.pack_uuid.lock().unwrap();
                    if pu != 0 {
                        let url = pack_url(state);
                        let ok = jni::push_pack(
                            env,
                            handle,
                            unsafe { as_obj(pu) },
                            &url,
                            &pack.sha1,
                            &state.cfg.pack_prompt,
                            false,
                        );
                        if ok {
                            st.pushed_sha = pack.sha1_hex.clone();
                        }
                    }
                }
            }
        }

        // 2. Tier switching — the only dynamic surface, pure /posteffect.
        let tier = tier_for(state, &p);
        let have_pack = pack.is_some();
        if let Some(handle) = jni::player_by_uuid(env, &p.uuid) {
            let want_terrain = have_pack && tier == Tier::Full && keys.terrain != 0;
            let want_atmo =
                have_pack && (tier == Tier::Full || tier == Tier::Veil) && keys.atmosphere != 0;

            if want_terrain && !st.terrain_on {
                st.terrain_on = jni::add_effect(env, handle, unsafe { as_obj(keys.terrain) });
            } else if !want_terrain && st.terrain_on {
                st.terrain_on = !jni::remove_effect(env, handle, unsafe { as_obj(keys.terrain) });
            }
            if want_atmo && !st.atmosphere_on {
                st.atmosphere_on = jni::add_effect(env, handle, unsafe { as_obj(keys.atmosphere) });
            } else if !want_atmo && st.atmosphere_on {
                st.atmosphere_on =
                    !jni::remove_effect(env, handle, unsafe { as_obj(keys.atmosphere) });
            }
        }

        state.per_player.lock().unwrap().insert(p.uuid.clone(), st);
    }

    // Drop state for players who left.
    state
        .per_player
        .lock()
        .unwrap()
        .retain(|k, _| seen.contains(k));
}

pub fn pack_url(state: &Arc<State>) -> String {
    let base = if state.cfg.pack_base_url.is_empty() {
        detect_base_url(state.cfg.http_port)
    } else {
        state.cfg.pack_base_url.clone()
    };
    format!("{base}/clod-pack.zip")
}

fn detect_base_url(port: u16) -> String {
    // Best-effort: pick the first non-loopback IPv4 via a UDP connect trick
    // (no packets are sent), else localhost.
    let ip = std::net::UdpSocket::bind("0.0.0.0:0")
        .and_then(|s| {
            s.connect("10.254.254.254:1")?;
            Ok(s)
        })
        .and_then(|s| s.local_addr())
        .map(|a| a.ip().to_string())
        .unwrap_or_else(|_| "127.0.0.1".to_string());
    format!("http://{ip}:{port}")
}

/// Engine-thread finalize driver. Runs `finalize` exactly once when the
/// static snapshot is complete (restored, swept fully, or deadline reached).
/// Pure Rust + HTTP swap: no JNI, safe off the main thread.
pub fn maybe_finalize(state: &Arc<State>) {
    if state.phase.load(Ordering::SeqCst) != PHASE_SCANNING {
        return;
    }
    // Wait for the atlas world so logs/coverage are coherent before the pack
    // goes live (one engine tick after world resolution at most).
    if *state.world_obj.lock().unwrap() == 0 {
        return;
    }
    let complete = state.atlas.scanned_cells() >= state.atlas.total_cells();
    let deadline_hit = match *state.world_seen_at.lock().unwrap() {
        Some(t) => t.elapsed().as_secs() >= state.cfg.scan_deadline_secs || state.restored,
        None => state.restored,
    };
    if !complete && !deadline_hit {
        return;
    }
    finalize(state);
}

/// Build the pack ONCE, swap it into the HTTP server, persist the atlas.
fn finalize(state: &Arc<State>) {
    // Exactly-once guarantee even under racy callers.
    if state
        .phase
        .compare_exchange(
            PHASE_SCANNING,
            PHASE_PACKING,
            Ordering::SeqCst,
            Ordering::SeqCst,
        )
        .is_err()
    {
        return;
    }
    let (scanned, total) = (state.atlas.scanned_cells(), state.atlas.total_cells());

    if state.cfg.atlas_cache {
        let path = std::path::Path::new(&state.cfg.atlas_path);
        match state.atlas.save(path) {
            Ok(()) => crate::log_info(&format!(
                "c-LoD atlas snapshot saved: {} ({} cells, {} bytes)",
                state.cfg.atlas_path,
                total,
                std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
            )),
            Err(e) => crate::log_info(&format!(
                "c-LoD atlas snapshot save FAILED ({e}) — restarts will rescan"
            )),
        }
    }

    let new_pack = packgen::build(&state.cfg, &state.atlas);
    state.server.set_pack(new_pack.zip.clone());
    crate::log_info(&format!(
        "c-LoD pack finalized (ONE-TIME, static): {scanned}/{total} cells ({}%), {} bytes, sha1 {}",
        scanned * 100 / total.max(1),
        new_pack.zip.len(),
        new_pack.sha1_hex
    ));
    *state.pack.write().unwrap() = Some(Arc::new(new_pack));
    state.phase.store(PHASE_FINALIZED, Ordering::SeqCst);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_url_uses_base() {
        let st = State::new(Config::parse("pack_base_url=http://x.example;http_port=1"));
        assert_eq!(pack_url(&st), "http://x.example/clod-pack.zip");
    }

    #[test]
    fn coverage_matches_world_and_bounds() {
        let st = State::new(Config::parse(
            "center_x=0;center_z=0;atlas_size=1024;cell_size=4",
        ));
        *st.world_name.lock().unwrap() = "world".to_string();
        let p = PlayerInfo {
            uuid: "u".into(),
            world: "world".into(),
            bx: 100,
            by: 64,
            bz: 100,
        };
        assert_eq!(tier_for(&st, &p), Tier::Full);
        let p_out = PlayerInfo {
            uuid: "u".into(),
            world: "world".into(),
            bx: 90_000,
            by: 64,
            bz: 0,
        };
        assert_eq!(tier_for(&st, &p_out), Tier::Veil);
        let p_world = PlayerInfo {
            uuid: "u".into(),
            world: "world_nether".into(),
            bx: 0,
            by: 64,
            bz: 0,
        };
        assert_eq!(tier_for(&st, &p_world), Tier::Off);
        let p_noworld = PlayerInfo {
            uuid: "u".into(),
            world: String::new(),
            bx: 0,
            by: 64,
            bz: 0,
        };
        assert_eq!(tier_for(&st, &p_noworld), Tier::Off);
    }

    #[test]
    fn finalize_happens_exactly_once() {
        let dir = std::env::temp_dir().join(format!("clod-eng-{}", std::process::id()));
        std::fs::create_dir_all(&dir).ok();
        let st = State::new(Config::parse(&format!(
            "atlas_size=512;cell_size=8;atlas_cache=true;atlas_path={}/a.bin",
            dir.display()
        )));
        st.phase.store(PHASE_SCANNING, Ordering::SeqCst);
        *st.world_obj.lock().unwrap() = 1; // pretend the world resolved
        *st.world_seen_at.lock().unwrap() = Some(std::time::Instant::now());
        st.atlas.set(0, 0, 70, false);

        // Deadline NOT reached yet, atlas incomplete -> no finalize.
        maybe_finalize(&st);
        assert_eq!(st.phase.load(Ordering::SeqCst), PHASE_SCANNING);
        assert!(st.pack.read().unwrap().is_none());

        // Deadline reached -> finalize; second call is a no-op.
        *st.world_seen_at.lock().unwrap() =
            Some(std::time::Instant::now() - std::time::Duration::from_secs(10_000));
        maybe_finalize(&st);
        assert_eq!(st.phase.load(Ordering::SeqCst), PHASE_FINALIZED);
        let sha1 = st.pack.read().unwrap().as_ref().unwrap().sha1_hex.clone();
        maybe_finalize(&st);
        assert_eq!(st.phase.load(Ordering::SeqCst), PHASE_FINALIZED);
        assert_eq!(st.pack.read().unwrap().as_ref().unwrap().sha1_hex, sha1);

        // HTTP server serves the finalized pack bytes.
        let served = st.server.pack_bytes();
        assert!(!served.is_empty());
        assert_eq!(
            served.len(),
            st.pack.read().unwrap().as_ref().unwrap().zip.len()
        );

        // Cache file exists and restores to the same SHA-1 (restart sim).
        let path = dir.join("a.bin");
        assert!(path.exists());
        drop(st);
        let st2 = State::new(Config::parse(&format!(
            "atlas_size=512;cell_size=8;atlas_cache=true;atlas_path={}/a.bin",
            dir.display()
        )));
        assert!(st2.restored);
        assert_eq!(st2.atlas.scanned_cells(), 1);
        st2.phase.store(PHASE_SCANNING, Ordering::SeqCst);
        *st2.world_obj.lock().unwrap() = 1;
        *st2.world_seen_at.lock().unwrap() = Some(std::time::Instant::now());
        maybe_finalize(&st2);
        assert_eq!(
            st2.pack.read().unwrap().as_ref().unwrap().sha1_hex,
            sha1,
            "restored session must reproduce the exact same pack SHA-1"
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
