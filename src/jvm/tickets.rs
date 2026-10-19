//! Chunk pinning: keep LOD cells' chunks loaded so their display entities
//! stay alive beyond the vanilla view distance.
//!
//! Backends:
//! * **nms** — `TicketStorage.addTicketWithRadius(PLAYER_LOADING, pos, 0)`:
//!   loads the chunk at FULL level without entity/block ticking, never
//!   expires, not persisted. All resolved through runtime probes with exact
//!   26.3 signatures; any probe failure degrades to the next backend.
//! * **forceload** — Bukkit `World#setChunkForceLoaded` (entity-ticking,
//!   persistent — documented side effect).
//! * **none** — never pin; displays only exist inside already-loaded chunks.

use crate::config::TicketsBackend;
use crate::jvm::bukkit::BukkitApi;
use crate::jvm::jniutil::*;
use cplug_sdk::classes::ClassRef;
use cplug_sdk::jni_util::with_attached;
use jvmti_bindings::prelude::*;
use std::collections::HashSet;
use std::sync::{Arc, Mutex, OnceLock};

pub struct TicketsApi {
    /// "nms" or "forceload" (the mode actually in effect).
    pub mode: &'static str,
    // NMS chain handles (resolved by the probe, used at pin/unpin time)
    m_get_handle: Option<jni::jmethodID>,
    m_get_chunk_source: Option<jni::jmethodID>,
    f_ticket_storage: Option<jni::jfieldID>,
    m_add_radius: Option<jni::jmethodID>,
    m_remove_radius: Option<jni::jmethodID>,
    ticket_type_obj: Option<jni::jobject>, // global ref
    chunk_pos: Option<ClassRef>,
    m_chunk_pos_new: Option<jni::jmethodID>,
    /// Diagnostics from the probe (for the boot log line).
    pub note: String,
}

static TICKETS: OnceLock<Arc<TicketsApi>> = OnceLock::new();

// SAFETY: TicketsApi holds only JVM-wide handles (classes global refs,
// jmethodID/jfieldID) that are valid from any attached thread per the JNI
// spec — the same contract the SDK relies on for ClassRef.
unsafe impl Send for TicketsApi {}
unsafe impl Sync for TicketsApi {}

pub fn api() -> Option<&'static Arc<TicketsApi>> {
    TICKETS.get()
}

/// Probe the NMS chain (main thread). Falls back per config.
pub fn bootstrap(preferred: TicketsBackend) -> Result<(), String> {
    if TICKETS.get().is_some() {
        return Ok(());
    }
    if preferred == TicketsBackend::None {
        let _ = TICKETS.set(Arc::new(TicketsApi {
            mode: "none",
            m_get_handle: None,
            m_get_chunk_source: None,
            f_ticket_storage: None,
            m_add_radius: None,
            m_remove_radius: None,
            ticket_type_obj: None,
            chunk_pos: None,
            m_chunk_pos_new: None,
            note: "disabled by config".into(),
        }));
        return Ok(());
    }
    if preferred == TicketsBackend::ForceLoad {
        let _ = TICKETS.set(Arc::new(TicketsApi {
            mode: "forceload",
            note: "forced by config".into(),
            m_get_handle: None,
            m_get_chunk_source: None,
            f_ticket_storage: None,
            m_add_radius: None,
            m_remove_radius: None,
            ticket_type_obj: None,
            chunk_pos: None,
            m_chunk_pos_new: None,
        }));
        return Ok(());
    }

    let probed = with_attached(probe_nms).flatten();
    match probed {
        Some(Ok(t)) => {
            let _ = TICKETS.set(Arc::new(t));
            Ok(())
        }
        Some(Err(e)) => {
            let _ = TICKETS.set(Arc::new(TicketsApi {
                mode: "forceload",
                note: format!("nms probe failed ({e}) — forceload fallback"),
                m_get_handle: None,
                m_get_chunk_source: None,
                f_ticket_storage: None,
                m_add_radius: None,
                m_remove_radius: None,
                ticket_type_obj: None,
                chunk_pos: None,
                m_chunk_pos_new: None,
            }));
            Ok(())
        }
        None => {
            let _ = TICKETS.set(Arc::new(TicketsApi {
                mode: "forceload",
                note: "nms probe could not attach JNI — forceload fallback".into(),
                m_get_handle: None,
                m_get_chunk_source: None,
                f_ticket_storage: None,
                m_add_radius: None,
                m_remove_radius: None,
                ticket_type_obj: None,
                chunk_pos: None,
                m_chunk_pos_new: None,
            }));
            Ok(())
        }
    }
}

/// Loader-backed class resolution (NMS classes load lazily too).
fn ncls(env: &JniEnv, name: &str) -> Result<ClassRef, String> {
    super::bukkit::load_class(env, name)
}

fn probe_nms(env: &JniEnv) -> Option<Result<TicketsApi, String>> {
    let mut errs: Vec<String> = Vec::new();
    macro_rules! try_step {
        ($e:expr, $what:expr) => {
            match $e {
                Ok(v) => v,
                Err(e) => {
                    errs.push(format!("{}: {}", $what, e));
                    return Some(Err(errs.join("; ")));
                }
            }
        };
    }

    let craft_world = match ncls(env, "org/bukkit/craftbukkit/CraftWorld") {
        Ok(c) => c,
        Err(e) => {
            return Some(Err(format!(
                "class not loaded: org/bukkit/craftbukkit/CraftWorld: {e}"
            )))
        }
    };
    let m_get_handle = try_step!(
        method(
            env,
            craft_world.as_jclass(),
            "getHandle",
            "()Lnet/minecraft/server/level/ServerLevel;"
        ),
        "CraftWorld"
    );
    let server_level = match ncls(env, "net/minecraft/server/level/ServerLevel") {
        Ok(c) => c,
        Err(e) => {
            errs.push(format!("class not loaded: ServerLevel: {e}"));
            return Some(Err(errs.join("; ")));
        }
    };
    let m_get_chunk_source = try_step!(
        method(
            env,
            server_level.as_jclass(),
            "getChunkSource",
            "()Lnet/minecraft/server/level/ServerChunkCache;"
        ),
        "ServerLevel"
    );
    let chunk_cache = match ncls(env, "net/minecraft/server/level/ServerChunkCache") {
        Ok(c) => c,
        Err(e) => {
            errs.push(format!("class not loaded: ServerChunkCache: {e}"));
            return Some(Err(errs.join("; ")));
        }
    };
    let f_ticket_storage = try_step!(
        field(
            env,
            chunk_cache.as_jclass(),
            "ticketStorage",
            "Lnet/minecraft/world/level/TicketStorage;"
        ),
        "ServerChunkCache"
    );
    let ticket_storage = match ncls(env, "net/minecraft/world/level/TicketStorage") {
        Ok(c) => c,
        Err(e) => {
            errs.push(format!("class not loaded: TicketStorage: {e}"));
            return Some(Err(errs.join("; ")));
        }
    };
    let m_add_radius = try_step!(
        method(
            env,
            ticket_storage.as_jclass(),
            "addTicketWithRadius",
            "(Lnet/minecraft/server/level/TicketType;Lnet/minecraft/world/level/ChunkPos;I)V"
        ),
        "TicketStorage"
    );
    let m_remove_radius = try_step!(
        method(
            env,
            ticket_storage.as_jclass(),
            "removeTicketWithRadius",
            "(Lnet/minecraft/server/level/TicketType;Lnet/minecraft/world/level/ChunkPos;I)V"
        ),
        "TicketStorage"
    );

    // pick a ticket type: loading, non-simulating, never expires
    let ticket_type = match ncls(env, "net/minecraft/server/level/TicketType") {
        Ok(c) => c,
        Err(e) => {
            errs.push(format!("class not loaded: TicketType: {e}"));
            return Some(Err(errs.join("; ")));
        }
    };
    let mut chosen: Option<(jni::jobject, String)> = None;
    let mut type_note = String::new();
    for cand in ["PLAYER_LOADING", "PORTAL", "FORCED"] {
        let fid = match env.get_static_field_id(
            ticket_type.as_jclass(),
            cand,
            "Lnet/minecraft/server/level/TicketType;",
        ) {
            Some(f) => f,
            None => {
                swallow(env, "TicketType field");
                continue;
            }
        };
        let obj = env.get_static_object_field(ticket_type.as_jclass(), fid);
        swallow(env, "TicketType static read");
        if obj.is_null() {
            continue;
        }
        let loads = find_bool(env, ticket_type.as_jclass(), &obj, "doesLoad");
        let simulates = find_bool(env, ticket_type.as_jclass(), &obj, "doesSimulate");
        let times_out = find_bool(env, ticket_type.as_jclass(), &obj, "hasTimeout");
        type_note.push_str(&format!(
            " {}(load={},sim={},timeout={})",
            cand.to_lowercase(),
            loads,
            simulates,
            times_out
        ));
        if loads && !simulates && !times_out {
            let g = env.new_global_ref(obj);
            env.delete_local_ref(obj);
            chosen = Some((g, cand.to_string()));
            break;
        }
        env.delete_local_ref(obj);
    }
    let (type_obj, type_name) = match chosen {
        Some(v) => v,
        None => return Some(Err(format!("no suitable TicketType (flags:{})", type_note))),
    };

    let chunk_pos = match ncls(env, "net/minecraft/world/level/ChunkPos") {
        Ok(c) => c,
        Err(e) => {
            errs.push(format!("class not loaded: ChunkPos: {e}"));
            return Some(Err(errs.join("; ")));
        }
    };
    let m_chunk_pos_new = try_step!(
        method(env, chunk_pos.as_jclass(), "<init>", "(II)V"),
        "ChunkPos"
    );

    // pin level: FULL_CHUNK_LEVEL validated against ChunkLevel.fullStatus
    let chunk_level = match ncls(env, "net/minecraft/server/level/ChunkLevel") {
        Ok(c) => c,
        Err(e) => {
            errs.push(format!("class not loaded: ChunkLevel: {e}"));
            return Some(Err(errs.join("; ")));
        }
    };
    let f_full_chunk_level = try_step!(
        field(env, chunk_level.as_jclass(), "FULL_CHUNK_LEVEL", "I"),
        "ChunkLevel"
    );
    let level = env.get_static_int_field(chunk_level.as_jclass(), f_full_chunk_level);
    swallow(env, "ChunkLevel.FULL_CHUNK_LEVEL");
    let full_chunk_status = match ncls(env, "net/minecraft/server/level/FullChunkStatus") {
        Ok(c) => c,
        Err(e) => {
            errs.push(format!("class not loaded: FullChunkStatus: {e}"));
            return Some(Err(errs.join("; ")));
        }
    };
    let f_status_full = try_step!(
        static_field(
            env,
            full_chunk_status.as_jclass(),
            "FULL",
            "Lnet/minecraft/server/level/FullChunkStatus;"
        ),
        "FullChunkStatus"
    );
    let m_full_status = try_step!(
        method(
            env,
            chunk_level.as_jclass(),
            "fullStatus",
            "(I)Lnet/minecraft/server/level/FullChunkStatus;"
        ),
        "ChunkLevel"
    );
    let mut pin_level = level;
    let status =
        env.call_static_object_method(chunk_level.as_jclass(), m_full_status, &[jv_i(level)]);
    swallow(env, "ChunkLevel.fullStatus");
    let status_full = env.get_static_object_field(full_chunk_status.as_jclass(), f_status_full);
    let ok = !status.is_null() && !status_full.is_null() && env.is_same_object(status, status_full);
    if !status.is_null() {
        env.delete_local_ref(status);
    }
    if !ok {
        // scan nearby levels for the first one mapping to FULL
        let mut found = None;
        for l in 33..=40 {
            let st =
                env.call_static_object_method(chunk_level.as_jclass(), m_full_status, &[jv_i(l)]);
            if !st.is_null() {
                let same = !status_full.is_null() && env.is_same_object(st, status_full);
                env.delete_local_ref(st);
                if same {
                    found = Some(l);
                    break;
                }
            } else {
                swallow(env, "fullStatus scan");
            }
        }
        match found {
            Some(l) => {
                pin_level = l;
                errs.push(format!(
                    "FULL_CHUNK_LEVEL {} != FULL, using level {}",
                    level, l
                ));
            }
            None => return Some(Err("could not determine FULL chunk level".into())),
        }
    }

    Some(Ok(TicketsApi {
        mode: "nms",
        m_get_handle: Some(m_get_handle),
        m_get_chunk_source: Some(m_get_chunk_source),
        f_ticket_storage: Some(f_ticket_storage),
        m_add_radius: Some(m_add_radius),
        m_remove_radius: Some(m_remove_radius),
        ticket_type_obj: Some(type_obj),
        chunk_pos: Some(chunk_pos),
        m_chunk_pos_new: Some(m_chunk_pos_new),
        note: format!(
            "nms ok: type={}, level={}{}",
            type_name.to_lowercase(),
            pin_level,
            if errs.is_empty() {
                String::new()
            } else {
                format!(" ({})", errs.join("; "))
            }
        ),
    }))
}

fn find_bool(env: &JniEnv, cls: jni::jclass, obj: &jni::jobject, name: &str) -> bool {
    match env.get_method_id(cls, name, "()Z") {
        Some(mid) => call_bool(env, *obj, mid, &[], name),
        None => {
            swallow(env, name);
            false
        }
    }
}

// ---------------------------------------------------------------------------
// Pin/unpin (main thread)
// ---------------------------------------------------------------------------

/// Track pin failures per chunk to avoid log spam.
static PIN_FAILED: OnceLock<Mutex<HashSet<(String, i32, i32)>>> = OnceLock::new();

fn pin_failed() -> &'static Mutex<HashSet<(String, i32, i32)>> {
    PIN_FAILED.get_or_init(|| Mutex::new(HashSet::new()))
}

pub fn pin(bapi: &BukkitApi, env: &JniEnv, world: &str, cx: i32, cz: i32) -> bool {
    let t = match api() {
        Some(t) => t,
        None => return false,
    };
    match t.mode {
        "nms" => pin_nms(t, bapi, env, world, cx, cz),
        "forceload" => pin_forceload(bapi, env, world, cx, cz, true),
        _ => false,
    }
}

pub fn unpin(bapi: &BukkitApi, env: &JniEnv, world: &str, cx: i32, cz: i32) -> bool {
    let t = match api() {
        Some(t) => t,
        None => return false,
    };
    match t.mode {
        "nms" => unpin_nms(t, bapi, env, world, cx, cz),
        "forceload" => pin_forceload(bapi, env, world, cx, cz, false),
        _ => false,
    }
}

fn world_storage(
    t: &TicketsApi,
    bapi: &BukkitApi,
    env: &JniEnv,
    world: &str,
) -> Option<jni::jobject> {
    let wobj = super::bukkit::world_obj(bapi, env, world)?;
    let handle = env.call_object_method(wobj, t.m_get_handle?, &[]);
    if swallow(env, "getHandle") || handle.is_null() {
        if !handle.is_null() {
            env.delete_local_ref(handle);
        }
        return None;
    }
    let cache = env.call_object_method(handle, t.m_get_chunk_source?, &[]);
    env.delete_local_ref(handle);
    if swallow(env, "getChunkSource") || cache.is_null() {
        if !cache.is_null() {
            env.delete_local_ref(cache);
        }
        return None;
    }
    let storage = env.get_object_field(cache, t.f_ticket_storage?);
    swallow(env, "ticketStorage field");
    env.delete_local_ref(cache);
    if storage.is_null() {
        None
    } else {
        Some(storage)
    }
}

fn ticket_pos(t: &TicketsApi, env: &JniEnv, cx: i32, cz: i32) -> Option<jni::jobject> {
    let pos = env
        .new_object(
            t.chunk_pos.as_ref()?.as_jclass(),
            t.m_chunk_pos_new?,
            &[jv_i(cx), jv_i(cz)],
        )
        .unwrap_or(std::ptr::null_mut());
    if pos.is_null() {
        swallow(env, "new ChunkPos");
        None
    } else {
        Some(pos)
    }
}

fn pin_nms(t: &TicketsApi, bapi: &BukkitApi, env: &JniEnv, world: &str, cx: i32, cz: i32) -> bool {
    let Some(storage) = world_storage(t, bapi, env, world) else {
        return log_pin_fail(world, cx, cz, "no TicketStorage");
    };
    let Some(pos) = ticket_pos(t, env, cx, cz) else {
        env.delete_local_ref(storage);
        return log_pin_fail(world, cx, cz, "no ChunkPos");
    };
    let Some(tt) = t.ticket_type_obj else {
        env.delete_local_ref(pos);
        env.delete_local_ref(storage);
        return log_pin_fail(world, cx, cz, "no TicketType");
    };
    let Some(add) = t.m_add_radius else {
        env.delete_local_ref(pos);
        env.delete_local_ref(storage);
        return log_pin_fail(world, cx, cz, "no addTicketWithRadius");
    };
    let ok = call_void(
        env,
        storage,
        add,
        &[jv_o(tt), jv_o(pos), jv_i(0)],
        "addTicketWithRadius",
    );
    env.delete_local_ref(pos);
    env.delete_local_ref(storage);
    if !ok {
        log_pin_fail(world, cx, cz, "addTicketWithRadius threw");
    }
    ok
}

fn unpin_nms(
    t: &TicketsApi,
    bapi: &BukkitApi,
    env: &JniEnv,
    world: &str,
    cx: i32,
    cz: i32,
) -> bool {
    let Some(storage) = world_storage(t, bapi, env, world) else {
        return false;
    };
    let Some(pos) = ticket_pos(t, env, cx, cz) else {
        env.delete_local_ref(storage);
        return false;
    };
    let Some(tt) = t.ticket_type_obj else {
        env.delete_local_ref(pos);
        env.delete_local_ref(storage);
        return false;
    };
    let Some(remove) = t.m_remove_radius else {
        env.delete_local_ref(pos);
        env.delete_local_ref(storage);
        return false;
    };
    let ok = call_void(
        env,
        storage,
        remove,
        &[jv_o(tt), jv_o(pos), jv_i(0)],
        "removeTicketWithRadius",
    );
    env.delete_local_ref(pos);
    env.delete_local_ref(storage);
    ok
}

fn pin_forceload(bapi: &BukkitApi, env: &JniEnv, world: &str, cx: i32, cz: i32, on: bool) -> bool {
    let Some(wobj) = super::bukkit::world_obj(bapi, env, world) else {
        return log_pin_fail(world, cx, cz, "no world for forceload");
    };
    let ok = call_void(
        env,
        wobj,
        bapi.m_world_forceload,
        &[jv_i(cx), jv_i(cz), jv_z(on)],
        "setChunkForceLoaded",
    );
    if !ok {
        log_pin_fail(world, cx, cz, "setChunkForceLoaded threw");
    }
    ok
}

fn log_pin_fail(world: &str, cx: i32, cz: i32, why: &str) -> bool {
    let mut set = pin_failed().lock().unwrap();
    if set.insert((world.to_string(), cx, cz)) {
        eprintln!("[c-LoD] pin failed {world} ({cx},{cz}): {why}");
    }
    false
}
