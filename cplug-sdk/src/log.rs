//! Kernel logging via `java.util.logging.Logger` (bootstrap class, resolvable
//! with plain JNI — no GetLoadedClasses needed). Falls back to stderr until
//! the kernel is up.

use crate::classes::{find_class, method, static_method, ClassRef};
use crate::jni_util::with_attached;
use jvmti_bindings::prelude::*;
use std::sync::{Mutex, OnceLock};

enum Level {
    Info,
    Warning,
}

struct LoggerIds {
    bukkit: ClassRef,
    get_logger: usize,
    info: usize,
    warning: usize,
}

static IDS: OnceLock<Mutex<Option<LoggerIds>>> = OnceLock::new();

fn ids() -> &'static Mutex<Option<LoggerIds>> {
    IDS.get_or_init(|| Mutex::new(None))
}

fn resolve() -> Option<LoggerIds> {
    let bukkit = find_class("org/bukkit/Bukkit")?;
    let logger = find_class("java/util/logging/Logger")?;
    with_attached(|env| {
        let get_logger = static_method(
            env,
            bukkit.as_jclass(),
            "getLogger",
            "()Ljava/util/logging/Logger;",
        )?;
        let info = method(env, logger.as_jclass(), "info", "(Ljava/lang/String;)V")?;
        let warning = method(env, logger.as_jclass(), "warning", "(Ljava/lang/String;)V")?;
        Some(LoggerIds {
            bukkit,
            get_logger,
            info,
            warning,
        })
    })
    .flatten()
}

fn emit(level: Level, msg: &str) {
    let mut cache = ids().lock().unwrap();
    if cache.is_none() {
        *cache = resolve();
    }
    let Some(resolved) = cache.as_ref() else {
        eprintln!("[cplug-sdk] log dropped (kernel not ready): {msg}");
        return;
    };
    let mid = match level {
        Level::Info => resolved.info,
        Level::Warning => resolved.warning,
    } as jni::jmethodID;
    let get_logger = resolved.get_logger as jni::jmethodID;
    let bukkit = resolved.bukkit.as_jclass();
    // (retry_after_failure is returned from the attached closure so the
    // cache guard is never re-locked inside it — that would self-deadlock.)
    let outcome: Option<(bool /*served*/, bool /*drop_ids*/)> = with_attached(|env| {
        // Bukkit.getLogger() NPEs while Bukkit.server is still null (the
        // kernel-ready trigger fires on class LOAD, long before Paper calls
        // setServer). A Java exception escaping a native frame here would
        // kill the caller's thread (or the server tick) — never let it out.
        let done = (|| -> Option<(bool, bool)> {
            let s = env.new_string_utf(msg)?;
            let logger = env.call_static_object_method(bukkit, get_logger, &[]);
            if env.exception_check() {
                // Server not assigned yet (or the call failed): clear, drop
                // the resolved ids so the next emit retries, and let the
                // stderr fallback handle this message.
                env.exception_clear();
                env.delete_local_ref(s);
                return Some((false, true));
            }
            if logger.is_null() {
                env.delete_local_ref(s);
                return Some((false, false));
            }
            env.call_void_method(logger, mid, &[jni::jvalue { l: s }]);
            let failed = env.exception_check();
            env.delete_local_ref(s);
            env.delete_local_ref(logger);
            if failed {
                env.exception_clear();
                return Some((false, false));
            }
            Some((true, false))
        })();
        done
    })
    .flatten();
    if outcome.map(|(served, drop_ids)| {
        if drop_ids {
            *cache = None;
        }
        served
    }) != Some(true)
    {
        eprintln!("[cplug-sdk] log emit failed: {msg}");
    }
}

/// Log an info message through the kernel logger (stderr fallback before the
/// kernel is up).
pub fn info(msg: &str) {
    emit(Level::Info, msg);
}

/// Log a warning through the kernel logger.
pub fn warn(msg: &str) {
    emit(Level::Warning, msg);
}

pub fn log_info(msg: &str) {
    info(msg);
}
