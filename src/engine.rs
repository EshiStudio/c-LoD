//! The c-LoD engine: a state machine driven from the server main thread.
//!
//! Per engine tick (default 1s):
//!   1. Scan a budgeted batch of atlas cells (JNI getHighestBlockYAt).
//!   2. For every online player: ensure the resource pack is pushed, then
//!      add/remove the two post-effect chains based on atlas coverage.
//!
//! A background thread rebuilds the resource pack when the atlas changed
//! and swaps it into the HTTP server; the next tick re-pushes the pack.

use crate::atlas::Atlas;
use crate::config::{self, Config};
use crate::http::PackServer;
use crate::jni::{self, PlayerInfo};
use crate::packgen::{self, Pack};
use jvmti_bindings::prelude::JniEnv;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

pub struct State {
    pub cfg: Config,
    pub atlas: Arc<Atlas>,
    pub server: Arc<PackServer>,
    pub pack: RwLock<Option<Arc<Pack>>>,
    /// Set when a new pack was swapped in and clients must re-push.
    pub repack_pending: AtomicBool,
    pub scan_seq: AtomicU64,
    pub world_obj: Mutex<usize>, // jobject handle as usize; main-thread only
    pub world_name: Mutex<String>,
    pub keys: Mutex<Option<Keys>>,
    pub per_player: Mutex<HashMap<String, PlayerState>>,
    pub last_repack: Mutex<std::time::Instant>,
    pub pack_uuid: Mutex<usize>,
    /// Consecutive adaptive ticks with every player outside the window.
    pub recenter_streak: AtomicU32,
}

#[derive(Clone)]
pub struct Keys {
    pub terrain: usize, // jobject global refs (main-thread only)
    /// Atmosphere veil tier keys, weakest tier first (index = tier).
    pub atmo: Vec<usize>,
}

#[derive(Default, Clone)]
pub struct PlayerState {
    pub pushed_sha: String,
    pub terrain_on: bool,
    /// Currently applied veil tier; None = none applied.
    pub atmo_tier: Option<u32>,
}

impl State {
    pub fn new(cfg: Config) -> Arc<Self> {
        let atlas = Arc::new(Atlas::new(
            cfg.center_x,
            cfg.center_z,
            cfg.atlas_size,
            cfg.cell_size,
            cfg.sea_level,
        ));
        let server = PackServer::new();
        Arc::new(Self {
            atlas,
            server,
            pack: RwLock::new(None),
            repack_pending: AtomicBool::new(false),
            scan_seq: AtomicU64::new(0),
            world_obj: Mutex::new(0),
            world_name: Mutex::new(String::new()),
            keys: Mutex::new(None),
            per_player: Mutex::new(HashMap::new()),
            last_repack: Mutex::new(std::time::Instant::now()),
            pack_uuid: Mutex::new(0),
            recenter_streak: AtomicU32::new(0),
            cfg,
        })
    }
}

type Env<'a> = &'a JniEnv;

/// One engine tick. MUST run on the main thread.
pub fn tick(state: &Arc<State>, env: Env) {
    ensure_boot(state, env);
    scan_batch(state, env);
    manage_players(state, env);
}

fn ensure_boot(state: &Arc<State>, env: Env) {
    // Resolve JNI ids once.
    if crate::jni_warm_marker()
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
    {
        let ok = jni::warmup(env);
        crate::log_info(&format!("c-LoD JNI bridge resolved: {ok}"));
        if !ok {
            crate::log_info("c-LoD: JNI surface incomplete — is this Paper 26.3+? Module idles.");
        }
        // Effect keys (global refs, main thread). Atmosphere keys mirror the
        // tier ids packgen emits: `{effect_atmosphere}_t{i}`, weakest first.
        if ok {
            let terrain = jni::make_key(env, &state.cfg.effect_terrain)
                .map(|k| unsafe { new_global(env, k) })
                .unwrap_or(0);
            let tiers = state.cfg.atmo_tiers.max(1);
            let mut atmo = Vec::with_capacity(tiers as usize);
            for t in 0..tiers {
                let id = format!("{}_t{t}", state.cfg.effect_atmosphere);
                let key = jni::make_key(env, &id)
                    .map(|k| unsafe { new_global(env, k) })
                    .unwrap_or(0);
                atmo.push(key);
            }
            *state.keys.lock().unwrap() = Some(Keys { terrain, atmo });
            // Pack UUID global.
            let uuid = jni::parse_uuid(env, &state.cfg.pack_uuid)
                .map(|u| unsafe { new_global(env, u) })
                .unwrap_or(0);
            *state.pack_uuid.lock().unwrap() = uuid;
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
    let world_handle = *state.world_obj.lock().unwrap();
    if world_handle == 0 {
        return;
    }
    let world = unsafe { as_obj(world_handle) };
    let seq = state.scan_seq.fetch_add(1, Ordering::Relaxed);
    let budget = state.cfg.scan_budget;
    let tasks = state.atlas.scan_task(seq, budget);
    let mut changed = false;
    for (ix, iz) in tasks {
        let x = state.atlas.cell_world_x(ix);
        let z = state.atlas.cell_world_z(iz);
        if let Some((y, liquid)) = jni::sample_height(env, world, x, z) {
            let before = state.atlas.version();
            state.atlas.set(ix, iz, (y.clamp(-64, 1023)) as i16, liquid);
            changed |= state.atlas.version() != before;
        }
    }
    let _ = changed;
}

fn coverage(state: &Arc<State>, p: &PlayerInfo) -> bool {
    if p.world.is_empty() {
        return false;
    }
    let wname = state.world_name.lock().unwrap().clone();
    if !wname.is_empty() && p.world != wname {
        return false;
    }
    state.atlas.cell_index(p.bx, p.bz).is_some()
}

/// Desired veil tier for a player: `Some(tier)` when covered (tier index
/// into `Keys.atmo`, weakest first), `None` when effects must be off.
/// Adaptive mode reads the scanned surface height under the player; the
/// higher above ground they are, the weaker the veil (see further).
pub fn desired_tier(state: &Arc<State>, p: &PlayerInfo) -> Option<u32> {
    if !coverage(state, p) {
        return None;
    }
    let max = state.cfg.atmo_tiers.max(1) - 1;
    if !state.cfg.adaptive {
        return Some(max);
    }
    let ground = state
        .atlas
        .height_at(p.bx, p.bz)
        .map(i32::from)
        .unwrap_or(state.cfg.sea_level);
    Some(config::tier_from_delta(p.by - ground, max))
}

fn manage_players(state: &Arc<State>, env: Env) {
    let keys = state.keys.lock().unwrap().clone();
    let Some(keys) = keys else { return };
    if keys.terrain == 0 && keys.atmo.iter().all(|k| *k == 0) {
        return;
    }
    let pack = state.pack.read().unwrap().clone();
    let Some(pack) = pack else { return };
    let repack = state.repack_pending.load(Ordering::Relaxed);

    let url = pack_url(state);
    let players = jni::players(env);
    let mut seen: Vec<String> = Vec::new();

    for p in &players {
        seen.push(p.uuid.clone());
        let mut st = state
            .per_player
            .lock()
            .unwrap()
            .entry(p.uuid.clone())
            .or_default()
            .clone();

        // Resource pack (re-)push.
        let need_push = repack || st.pushed_sha.is_empty() || st.pushed_sha != pack.sha1_hex;
        if need_push {
            if let Some(handle) = jni::player_by_uuid(env, &p.uuid) {
                let pu = *state.pack_uuid.lock().unwrap();
                if pu != 0 {
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

        // Terrain chain by coverage; veil tier by altitude adaptivity.
        let want = coverage(state, p);
        let tier = desired_tier(state, p);
        if let Some(handle) = jni::player_by_uuid(env, &p.uuid) {
            if want {
                if keys.terrain != 0 && !st.terrain_on {
                    st.terrain_on = jni::add_effect(env, handle, unsafe { as_obj(keys.terrain) });
                }
            } else if st.terrain_on {
                st.terrain_on = !jni::remove_effect(env, handle, unsafe { as_obj(keys.terrain) });
            }

            // Atmosphere: switch tiers only when the tier actually changes,
            // so the client never sees add/remove spam. Skipped entirely
            // when no tier keys resolved.
            if !keys.atmo.is_empty() {
                let atmo_max = keys.atmo.len() - 1;
                match (tier, st.atmo_tier) {
                    (Some(want_t), cur_t) if cur_t != Some(want_t) => {
                        if let Some(cur) = cur_t {
                            let key = keys.atmo[(cur as usize).min(atmo_max)];
                            if key != 0 {
                                let off = jni::remove_effect(env, handle, unsafe { as_obj(key) });
                                if off {
                                    st.atmo_tier = None;
                                }
                            }
                        }
                        if st.atmo_tier.is_none() {
                            let key = keys.atmo[(want_t as usize).min(atmo_max)];
                            if key != 0 && jni::add_effect(env, handle, unsafe { as_obj(key) }) {
                                st.atmo_tier = Some(want_t);
                            }
                        }
                    }
                    (None, Some(cur)) => {
                        let key = keys.atmo[(cur as usize).min(atmo_max)];
                        if key != 0 && jni::remove_effect(env, handle, unsafe { as_obj(key) }) {
                            st.atmo_tier = None;
                        }
                    }
                    _ => {}
                }
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

    if repack {
        state.repack_pending.store(false, Ordering::Relaxed);
    }

    maybe_recenter(state, &players);
}

/// Follow the players: when every player in the atlas world has drifted
/// `recenter_blocks` (Chebyshev) outside the window for `recenter_patience`
/// consecutive ticks, move the window to their centroid and rescan. Runs on
/// the main thread; `snapshot` is this tick's online-player positions.
fn maybe_recenter(state: &Arc<State>, snapshot: &[PlayerInfo]) {
    if state.cfg.recenter_blocks == 0 || snapshot.is_empty() {
        state.recenter_streak.store(0, Ordering::Relaxed);
        return;
    }
    let wname = state.world_name.lock().unwrap().clone();
    let relevant: Vec<(i32, i32)> = snapshot
        .iter()
        .filter(|p| wname.is_empty() || p.world == wname)
        .map(|p| (p.bx, p.bz))
        .collect();
    if relevant.is_empty() {
        state.recenter_streak.store(0, Ordering::Relaxed);
        return;
    }
    let worst = relevant
        .iter()
        .map(|(x, z)| state.atlas.distance_outside(*x, *z))
        .max()
        .unwrap_or(0);
    if worst < state.cfg.recenter_blocks {
        state.recenter_streak.store(0, Ordering::Relaxed);
        return;
    }
    let streak = state.recenter_streak.fetch_add(1, Ordering::Relaxed) + 1;
    if streak >= state.cfg.recenter_patience.max(1) {
        let cx = relevant.iter().map(|(x, _)| x).sum::<i32>() / relevant.len() as i32;
        let cz = relevant.iter().map(|(_, z)| z).sum::<i32>() / relevant.len() as i32;
        state.atlas.recenter(cx, cz);
        state.recenter_streak.store(0, Ordering::Relaxed);
        crate::log_info(&format!(
            "c-LoD atlas recentered to ({cx}, {cz}); rescanning {} cells",
            state.atlas.total_cells()
        ));
    }
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

/// Background: rebuild the pack if the atlas changed enough since last time.
pub fn maybe_rebuild_pack(state: &Arc<State>) {
    let interval = std::time::Duration::from_secs(state.cfg.refresh_secs.min(60));
    {
        let last = state.last_repack.lock().unwrap();
        if last.elapsed() < interval {
            return;
        }
    }
    let pack = state.pack.read().unwrap().clone();
    let atlas_version = state.atlas.version();
    if let Some(p) = &pack {
        if p.atlas_version == atlas_version {
            return;
        }
        // First completion also matters: don't publish a 0% atlas.
        let scanned = state.atlas.scanned_cells();
        if scanned < state.atlas.total_cells() / 4 && p.atlas_version <= 1 {
            return;
        }
    } else {
        // Initial pack: wait for a fraction of the atlas.
        let scanned = state.atlas.scanned_cells();
        if scanned < state.atlas.total_cells() / 8 {
            return;
        }
    }
    *state.last_repack.lock().unwrap() = std::time::Instant::now();
    let new_pack = packgen::build(&state.cfg, &state.atlas);
    state.server.set_pack(new_pack.zip.clone());
    crate::log_info(&format!(
        "c-LoD pack rebuilt: atlas v{} -> {} bytes, sha1 {}, veil tiers [{}]",
        new_pack.atlas_version,
        new_pack.zip.len(),
        new_pack.sha1_hex,
        new_pack.atmosphere_ids.join(", ")
    ));
    *state.pack.write().unwrap() = Some(Arc::new(new_pack));
    state.repack_pending.store(true, Ordering::Relaxed);
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
        assert!(coverage(&st, &p));
        let p_out = PlayerInfo {
            uuid: "u".into(),
            world: "world".into(),
            bx: 90_000,
            by: 64,
            bz: 0,
        };
        assert!(!coverage(&st, &p_out));
        let p_world = PlayerInfo {
            uuid: "u".into(),
            world: "world_nether".into(),
            bx: 0,
            by: 64,
            bz: 0,
        };
        assert!(!coverage(&st, &p_world));
    }

    #[test]
    fn adaptive_tiers_follow_altitude() {
        let st = State::new(Config::parse(
            "center_x=0;center_z=0;atlas_size=1024;cell_size=4;adaptive=true;atmo_tiers=3",
        ));
        *st.world_name.lock().unwrap() = "world".to_string();
        st.atlas
            .set(st.atlas.cells / 2, st.atlas.cells / 2, 64, false);
        let in_world = |by: i32| PlayerInfo {
            uuid: "u".into(),
            world: "world".into(),
            bx: 0,
            by,
            bz: 0,
        };
        // Ground level -> strongest veil (tier 2); high above -> weakest (0).
        assert_eq!(desired_tier(&st, &in_world(64)), Some(2));
        assert_eq!(desired_tier(&st, &in_world(96)), Some(1));
        assert_eq!(desired_tier(&st, &in_world(200)), Some(0));
        // Outside coverage -> effects off.
        let out = PlayerInfo {
            uuid: "u".into(),
            world: "world".into(),
            bx: 90_000,
            by: 64,
            bz: 0,
        };
        assert_eq!(desired_tier(&st, &out), None);
    }

    #[test]
    fn non_adaptive_uses_strongest_tier() {
        let st = State::new(Config::parse(
            "center_x=0;center_z=0;atlas_size=1024;cell_size=4;adaptive=false;atmo_tiers=3",
        ));
        *st.world_name.lock().unwrap() = "world".to_string();
        let p = PlayerInfo {
            uuid: "u".into(),
            world: "world".into(),
            bx: 0,
            by: 200,
            bz: 0,
        };
        assert_eq!(desired_tier(&st, &p), Some(2));
    }

    #[test]
    fn tier_keys_match_pack_ids() {
        let cfg = Config::parse("atmo_tiers=4");
        let ids: Vec<String> = (0..cfg.atmo_tiers)
            .map(|t| format!("{}_t{t}", cfg.effect_atmosphere))
            .collect();
        assert_eq!(ids[0], "clod:atmosphere_t0");
        assert_eq!(ids[3], "clod:atmosphere_t3");
    }
}
