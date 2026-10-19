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
use crate::config::Config;
use crate::http::PackServer;
use crate::jni::{self, PlayerInfo};
use crate::packgen::{self, Pack};
use jvmti_bindings::prelude::JniEnv;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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
}

#[derive(Clone)]
pub struct Keys {
    pub terrain: usize, // jobject global refs (main-thread only)
    pub atmosphere: usize,
}

#[derive(Default, Clone)]
pub struct PlayerState {
    pub pushed_sha: String,
    pub terrain_on: bool,
    pub atmosphere_on: bool,
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
        // Effect keys (global refs, main thread).
        if ok {
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

fn manage_players(state: &Arc<State>, env: Env) {
    let keys = state.keys.lock().unwrap().clone();
    let Some(keys) = keys else { return };
    if keys.terrain == 0 && keys.atmosphere == 0 {
        return;
    }
    let pack = state.pack.read().unwrap().clone();
    let Some(pack) = pack else { return };
    let repack = state.repack_pending.load(Ordering::Relaxed);

    let url = pack_url(state);
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

        // Post effects by coverage.
        let want = coverage(state, &p);
        if let Some(handle) = jni::player_by_uuid(env, &p.uuid) {
            if want {
                if keys.terrain != 0 && !st.terrain_on {
                    st.terrain_on = jni::add_effect(env, handle, unsafe { as_obj(keys.terrain) });
                }
                if keys.atmosphere != 0 && !st.atmosphere_on {
                    st.atmosphere_on =
                        jni::add_effect(env, handle, unsafe { as_obj(keys.atmosphere) });
                }
            } else {
                if st.terrain_on {
                    st.terrain_on =
                        !jni::remove_effect(env, handle, unsafe { as_obj(keys.terrain) });
                }
                if st.atmosphere_on {
                    st.atmosphere_on =
                        !jni::remove_effect(env, handle, unsafe { as_obj(keys.atmosphere) });
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
        "c-LoD pack rebuilt: atlas v{} -> {} bytes, sha1 {}",
        new_pack.atlas_version,
        new_pack.zip.len(),
        new_pack.sha1_hex
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
}
