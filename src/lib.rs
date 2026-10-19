//! c-LoD — Level-of-Distant-Detail terrain for Paper 26.3+ servers.
//!
//! A CRUSSTY module (native Rust, JVMTI-injected before the kernel boots)
//! that renders distant terrain as BlockDisplay impostors:
//!
//! 1. **Pre-boot** — load `clod.conf`, raise `entity-tracking-range.display`
//!    in spigot.yml so vanilla clients receive displays at LOD distances.
//! 2. **Kernel-ready** — probe the Bukkit + NMS surfaces once (exact 26.3
//!    signatures), report the verdict, start the engine worker.
//! 3. **Worker** — quadtree annulus planning, chunk snapshot aggregation,
//!    delta planning; feeds bounded op batches to the main thread.
//! 4. **Main thread** — spawn/update/despawn displays, pin/unpin chunks via
//!    `TicketStorage` (PLAYER_LOADING, FULL level — no ticking).

pub mod config;
mod engine;
mod jvm;
pub mod palette;
pub mod plan;
pub mod quadtree;
pub mod snapshot;
mod state;

use config::Config;
use cplug_sdk::jni_util::with_attached;
use jvmti_bindings::prelude::*;
use state::msg::{JvmOp, JvmResult};

/// The single required export (cplug-abi contract).
///
/// # Safety
/// `api` must be the CPluginApi owned by the runtime, `vm` the live JavaVM
/// pointer, `options` a NUL-terminated runtime options string.
#[no_mangle]
pub unsafe extern "C" fn cplugin_init(
    api: *const cplug_abi::CPluginApi,
    vm: cplug_abi::JavaVmPtr,
    options: *const std::ffi::c_char,
) -> i32 {
    eprintln!("[c-LoD] cplugin_init: native module injected (pre-kernel)");
    cplug_sdk::init(api, vm);

    // ---- runtime options -> module/server paths --------------------------
    let opts = read_options(options);
    let (module_dir, server_dir) = discover_dirs(&opts);
    eprintln!(
        "[c-LoD] options: modules={:?} server={:?}",
        module_dir.as_ref().map(|p| p.display().to_string()),
        server_dir.as_ref().map(|p| p.display().to_string()),
    );

    // ---- configuration ----------------------------------------------------
    let cfg = load_config(&module_dir);
    let max_blocks = cfg.max_distance;

    // ---- pre-boot config fixups (before the kernel reads spigot.yml) ------
    if cfg.tracking_raise {
        if let Some(sd) = &server_dir {
            let spigot = sd.join("spigot.yml");
            match jvm::spigotcfg::apply_to_file(&spigot, max_blocks) {
                Ok(note) => eprintln!("[c-LoD] spigot.yml: {note}"),
                Err(e) => eprintln!(
                    "[c-LoD] spigot.yml fixup failed: {e} (far displays may be invisible until entity-tracking-range.display >= {max_blocks})"
                ),
            }
        } else {
            eprintln!(
                "[c-LoD] server dir unknown — skipping spigot.yml fixup (need entity-tracking-range.display >= {max_blocks})"
            );
        }
    }

    if !state::install(cfg.clone()) {
        eprintln!("[c-LoD] state already installed (hot reload) — keeping existing config");
    }
    engine::start(cfg);
    engine::set_kick(Box::new(kick_main));

    // ---- kernel bootstrap ---------------------------------------------------
    cplug_sdk::on_kernel_ready("org.bukkit.Bukkit", || {
        eprintln!("[c-LoD] kernel ready — scheduling bootstrap on the main thread");
        cplug_sdk::run_on_main_thread(|_env| bootstrap_job());
    });
    0
}

fn read_options(options: *const std::ffi::c_char) -> String {
    if options.is_null() {
        return String::new();
    }
    let mut end = 0usize;
    unsafe {
        while end < 1024 && *options.add(end) != 0 {
            end += 1;
        }
        String::from_utf8_lossy(std::slice::from_raw_parts(options as *const u8, end)).into_owned()
    }
}

fn discover_dirs(opts: &str) -> (Option<std::path::PathBuf>, Option<std::path::PathBuf>) {
    let mut modules = None;
    let mut versions = None;
    for part in opts.split(';') {
        if let Some(v) = part.strip_prefix("modules=") {
            if !v.is_empty() {
                modules = Some(std::path::PathBuf::from(v));
            }
        } else if let Some(v) = part.strip_prefix("versions=") {
            if !v.is_empty() {
                versions = Some(std::path::PathBuf::from(v));
            }
        }
    }
    // server dir = parent of versions/ (the crussty server root)
    let server_dir = versions
        .as_ref()
        .and_then(|v| v.parent().map(|p| p.to_path_buf()));
    // module dir: <modules>/lod (directory layout of `crussty install`)
    let module_dir = modules
        .as_ref()
        .map(|m| m.join("lod"))
        .filter(|p| p.exists())
        .or(modules);
    (module_dir, server_dir)
}

fn load_config(module_dir: &Option<std::path::PathBuf>) -> Config {
    let candidates = [
        module_dir.as_ref().map(|d| d.join("clod.conf")),
        std::env::current_dir().ok().map(|d| d.join("clod.conf")),
    ];
    for path in candidates.into_iter().flatten() {
        if let Ok(text) = std::fs::read_to_string(&path) {
            match Config::parse(&text) {
                Ok(cfg) => {
                    eprintln!("[c-LoD] config loaded from {}", path.display());
                    return cfg;
                }
                Err(e) => {
                    eprintln!(
                        "[c-LoD] config error in {}: {e} — using defaults",
                        path.display()
                    );
                    return Config::default();
                }
            }
        }
    }
    eprintln!(
        "[c-LoD] no clod.conf found — using defaults (max_distance={} blocks)",
        Config::default().max_distance
    );
    Config::default()
}

// ---------------------------------------------------------------------------
// Main-thread jobs
// ---------------------------------------------------------------------------

/// One-time probe: resolve the Bukkit surface, pick the pinning backend,
/// report the view distance. Sets the engine in motion via `Boot`.
fn bootstrap_job() {
    let cfg = state::cfg();
    let boot = match jvm::bukkit::bootstrap() {
        Ok(()) => {
            let vd = with_attached(|env| {
                let api = jvm::bukkit::api().unwrap();
                let v =
                    env.call_static_int_method(api.bukkit.as_jclass(), api.m_view_distance, &[]);
                crate::jvm::jniutil::swallow(env, "getViewDistance");
                Some(v.max(0) as u32)
            })
            .flatten()
            .unwrap_or(0);
            let tickets_note = match jvm::tickets::bootstrap(cfg.tickets) {
                Ok(()) => match jvm::tickets::api() {
                    Some(t) => format!("tickets={}: {}", t.mode, t.note),
                    None => "tickets=?".into(),
                },
                Err(e) => format!("tickets probe error: {e}"),
            };
            eprintln!("[c-LoD] bootstrap ok: view_distance={vd} {tickets_note}");
            eprintln!(
                "[c-LoD] config: max_distance={} growth={} hysteresis={} interval={}ms budgets=(snap {},spawn {},update {},despawn {},pin {},unpin {})",
                cfg.max_distance,
                cfg.growth,
                cfg.hysteresis,
                cfg.update_interval_ms,
                cfg.snapshots_per_cycle,
                cfg.spawns_per_cycle,
                cfg.updates_per_cycle,
                cfg.despawns_per_cycle,
                cfg.pins_per_cycle,
                cfg.unpins_per_cycle,
            );
            JvmResult::Boot {
                view_distance: vd,
                note: tickets_note,
            }
        }
        Err(e) => {
            eprintln!("[c-LoD] BOOTSTRAP FAILED: {e} — engine idle (no LODs will render)");
            JvmResult::Boot {
                view_distance: 0,
                note: format!("FAILED: {e}"),
            }
        }
    };
    engine::send_result(boot);
    // kick the first drain
    cplug_sdk::run_on_main_thread(|_env| drain_main());
}

/// Execute all queued JVM ops on the main thread. Kicked once per worker
/// cycle; heavy per-op JVM work is bounded by the planner's budgets.
fn drain_main() {
    let Some(api) = jvm::bukkit::api() else {
        return; // bootstrap failed — nothing safe to do
    };
    let ops = engine::take_ops();
    if ops.is_empty() {
        return;
    }
    let cfg = state::cfg();
    let _ = with_attached(|env| {
        for op in ops {
            // per-op local-ref frame: nothing leaks across ops
            if env.push_local_frame(256).is_err() {
                crate::jvm::jniutil::swallow(env, "push_local_frame");
                continue;
            }
            execute_op(env, api, &cfg, op);
            env.pop_local_frame(std::ptr::null_mut());
        }
    });
}

fn execute_op(env: &JniEnv, api: &jvm::bukkit::BukkitApi, cfg: &Config, op: JvmOp) {
    match op {
        JvmOp::ReadPlayers => {
            let ps = jvm::bukkit::read_players(api, env);
            engine::send_result(JvmResult::Players(ps));
        }
        JvmOp::ReadSnapshot { world, cx, cz } => {
            let d = jvm::bukkit::read_snapshot(api, env, &world, cx, cz);
            engine::send_result(JvmResult::Snapshot(d));
        }
        JvmOp::Spawn { key, visual } => match jvm::ops::spawn_display(api, env, &key, &visual, cfg)
        {
            Ok(uuid) => {
                engine::send_result(JvmResult::Spawned { key, uuid });
            }
            Err(e) => {
                eprintln!(
                    "[c-LoD] spawn {} ({}) failed: {e}",
                    key.world,
                    cell_pos(&key)
                );
            }
        },
        JvmOp::Update { key, uuid, visual } => {
            if let Err(e) = jvm::ops::update_display(api, env, &uuid, &key, &visual) {
                eprintln!("[c-LoD] update failed ({e}) — respawning cell");
                engine::send_result(JvmResult::Dead(uuid));
            }
        }
        JvmOp::Despawn { uuid } => {
            if uuid.is_empty() {
                return;
            }
            if jvm::ops::despawn_display(api, env, &uuid).is_err() {
                engine::send_result(JvmResult::Dead(uuid));
            }
        }
        JvmOp::Pin { world, cx, cz } => {
            let ok = jvm::tickets::pin(api, env, &world, cx, cz);
            engine::send_result(JvmResult::PinAck {
                pin: true,
                ok,
                world,
                cx,
                cz,
            });
        }
        JvmOp::Unpin { world, cx, cz } => {
            let ok = jvm::tickets::unpin(api, env, &world, cx, cz);
            engine::send_result(JvmResult::PinAck {
                pin: false,
                ok,
                world,
                cx,
                cz,
            });
        }
    }
}

fn cell_pos(key: &quadtree::CellKey) -> String {
    format!("level={} ({},{})", key.level, key.cx, key.cz)
}

// The worker kicks the drain once per cycle: engine::start wires this via a
// small shim registered here so engine.rs stays JVM-free.
pub fn kick_main() {
    cplug_sdk::run_on_main_thread(|_env| drain_main());
}
