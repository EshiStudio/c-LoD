//! c-LoD — true server-side LOD for Paper 26.3, as a CRUSSTY module.
//!
//! Architecture (NO Display entities, NO client mods):
//!   1. The module scans world heightmaps into a texture atlas (JNI, main
//!      thread, budgeted per tick).
//!   2. It generates a server resource pack containing the `/posteffect`
//!      chains + GLSL shaders + the atlas PNG, and serves it over an
//!      embedded HTTP server.
//!   3. Players receive the pack (`setResourcePack`) and the module applies
//!      the chains per player (`PlayerPostEffects.add(Key)`).
//!   4. On the client, `clod:lod_terrain` recovers the view rotation from
//!      the vanilla sun/moon position (GameTime is a free data channel),
//!      raymarches the heightfield atlas over sky pixels and paints distant
//!      terrain. `clod:atmosphere_t<n>` hides the render-distance edge with
//!      a depth veil — rotation-free, works from the first frame.
//!   5. Adaptive LOD (v2.1): the veil is emitted in N tiers; per player the
//!      engine picks the tier from height above terrain — climb up and the
//!      horizon sharpens, stay low and the edge stays hidden. The atlas
//!      window follows the players when they all drift outside it.

mod atlas;
mod config;
mod engine;
mod http;
mod jni;
mod packgen;
mod png;
mod sha1;
mod zipw;

use cplug_abi::{CPluginApi, JavaVmPtr};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

static JNI_WARM: AtomicBool = AtomicBool::new(false);

pub(crate) fn jni_warm_marker() -> &'static AtomicBool {
    &JNI_WARM
}

pub(crate) fn log_info(msg: &str) {
    eprintln!("[c-LoD] {msg}");
    cplug_sdk::log::info(&format!("[c-LoD] {msg}"));
}

static STATE: std::sync::OnceLock<Arc<engine::State>> = std::sync::OnceLock::new();

fn state() -> Option<&'static Arc<engine::State>> {
    STATE.get()
}

/// The single required export (cplug-abi contract).
///
/// # Safety
/// `api` must be a valid CPluginApi owned by the agent, `vm` the live
/// JavaVM pointer, `options` a NUL-terminated string for the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cplugin_init(
    api: *const CPluginApi,
    vm: JavaVmPtr,
    options: *const std::ffi::c_char,
) -> i32 {
    cplug_sdk::init(api, vm);

    let opts = cplug_sdk::jni_util::cstr(options).unwrap_or("").to_string();
    let cfg = config::Config::parse(&opts);
    log_info(&format!(
        "booting: atlas {}x{} blocks (cell {}), http :{}, fov {}",
        cfg.atlas_size, cfg.atlas_size, cfg.cell_size, cfg.http_port, cfg.fov
    ));

    let st = engine::State::new(cfg);
    let _ = STATE.set(Arc::clone(&st));

    // Embedded HTTP server for the resource pack (own thread, no JVM).
    {
        let srv = Arc::clone(&st.server);
        let port = st.cfg.http_port;
        std::thread::Builder::new()
            .name("clod-http".into())
            .spawn(move || {
                if let Err(e) = srv.serve(port) {
                    eprintln!("[c-LoD] http server failed: {e}");
                }
            })
            .ok();
    }

    // Everything JVM-side waits until the kernel (Bukkit) is up.
    cplug_sdk::on_kernel_ready("org.bukkit.Bukkit", move || {
        log_info("kernel ready — starting engine loop");
        let st = state().expect("state").clone();
        let tick_ms = st.cfg.tick_ms;
        let repack_period = st.cfg.refresh_secs;

        std::thread::Builder::new()
            .name("clod-engine".into())
            .spawn(move || {
                let mut last_repack_check = std::time::Instant::now();
                loop {
                    std::thread::sleep(std::time::Duration::from_millis(tick_ms));
                    let st = state().expect("state").clone();
                    if last_repack_check.elapsed().as_secs() >= repack_period.min(60) {
                        engine::maybe_rebuild_pack(&st);
                        last_repack_check = std::time::Instant::now();
                    }
                    cplug_sdk::run_on_main_thread({
                        let st = Arc::clone(&st);
                        move |_env| {
                            engine::tick(&st, _env);
                        }
                    });
                }
            })
            .ok();
    });

    0
}
