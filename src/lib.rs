//! c-LoD — true server-side LOD for Paper 26.3, as a CRUSSTY module.
//!
//! Architecture (NO Display entities, NO client mods):
//!   1. The module scans world heightmaps into a texture atlas (JNI, main
//!      thread, budgeted per tick) until the sweep completes or the scan
//!      deadline fires. The atlas is frozen and (by default) persisted.
//!   2. It generates a server resource pack containing two `/posteffect`
//!      chains + GLSL shaders + the atlas PNG, and serves it over an
//!      embedded HTTP server. The pack is built EXACTLY ONCE per session —
//!      never rebuilt, never re-pushed with new bytes (no client reloads).
//!   3. Players receive the pack once (`setResourcePack`, stable SHA-1);
//!      across restarts the restored snapshot reproduces the same bytes.
//!   4. On the client, `clod:lod_terrain` recovers the view rotation from
//!      the vanilla sun/moon position (GameTime is a free data channel),
//!      raymarches the world-anchored heightfield atlas from the live
//!      camera every frame and paints distant terrain — LOD auto-updates
//!      as players move, with zero server-side pack work.
//!   5. `clod:atmosphere` hides the render-distance edge with a depth veil.
//!      Per tick the engine switches chains per player (Off / Veil / Full
//!      tiers) — the only dynamic part, pure `/posteffect`.

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
use std::sync::Arc;

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
    // CI/ops escape hatch: the CRUSSTY launcher hardcodes the agent options
    // string, so extra module knobs ride in via CLOD_OPTS ("k=v;k2=v2")
    // appended after the runtime's own keys (unknown keys are ignored by the
    // runtime's parser, ours are parsed below).
    let opts = match std::env::var("CLOD_OPTS") {
        Ok(extra) if !extra.trim().is_empty() => format!("{opts};{}", extra.trim()),
        _ => opts,
    };
    let cfg = config::Config::parse(&opts);
    log_info(&format!(
        "booting: atlas {}x{} blocks (cell {}), http :{}, fov {}, static pack (scan deadline {}s, cache {})",
        cfg.atlas_size,
        cfg.atlas_size,
        cfg.cell_size,
        cfg.http_port,
        cfg.fov,
        cfg.scan_deadline_secs,
        if cfg.atlas_cache { "on" } else { "off" }
    ));
    log_info(
        "c-LoD v3.0: static pack mode — one finalize per session, zero rebuilds, zero re-pushes",
    );

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

        std::thread::Builder::new()
            .name("clod-engine".into())
            .spawn(move || loop {
                std::thread::sleep(std::time::Duration::from_millis(tick_ms));
                let st = state().expect("state").clone();
                // Static pack: the one-time finalize runs here (pure Rust +
                // HTTP swap, no JNI). After it, this call is a cheap no-op.
                engine::maybe_finalize(&st);
                cplug_sdk::run_on_main_thread({
                    let st = Arc::clone(&st);
                    move |_env| {
                        engine::tick(&st, _env);
                    }
                });
            })
            .ok();
    });

    0
}
