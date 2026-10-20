//! JNI bridge to Paper 26.3 (Mojang-mapped, unobfuscated).
//!
//! Everything here runs on the server main thread via
//! `cplug_sdk::run_on_main_thread`. Verified call surface:
//!   - org.bukkit.Bukkit.getServer()            -> Server
//!   - Server.getOnlinePlayers()                -> Collection<Player>
//!   - Server.getWorlds()                       -> List<World>
//!   - World.getName()                          -> String
//!   - World.getHighestBlockYAt(int,int)        -> int
//!   - World.getBlockAt(int,int,int)            -> Block
//!   - Block.isLiquid()                         -> boolean
//!   - Player.getUniqueId()                     -> UUID
//!   - Player.getLocation()                     -> Location
//!   - Location.getBlockX/getBlockY/getBlockZ() -> int
//!   - Location.getWorld()                      -> World
//!   - Player.setResourcePack(UUID,String,byte[],String,boolean)
//!   - Player.postEffects()                     -> PlayerPostEffects
//!   - PlayerPostEffects.add/remove(Key)        -> boolean
//!   - Key.key(String)                          -> Key (static, adventure)
//!   - UUID.fromString(String)                  -> UUID (static)
//!
//! CLASS RESOLUTION RULES (hard-won against Paper 26.3's layered loaders):
//!   1. NEVER plain JNI FindClass from our callbacks — it sees only the
//!      system loader and a pending NoClassDefFoundError escaping the native
//!      frame kills the server tick.
//!   2. The JVMTI GetLoadedClasses scan finds classes across ALL loaders,
//!      but with several loaders hosting same-named API classes it can pick
//!      a duplicate whose method table mismatches (GetMethodID silently
//!      returns NULL). The scan is used ONLY for java.* bootstrap classes
//!      (single copy) and org/bukkit/Bukkit (verified against getServer).
//!   3. Everything else is resolved on the CONCRETE class of a live object
//!      (CraftServer / CraftWorld / CraftPlayer / CraftBlock / Location) via
//!      GetObjectClass — the exact class that actually implements the call.
//!      adventure Key and PlayerPostEffects come from Class.forName through
//!      CraftServer's own loader (exact same-loader Class object).

use jvmti_bindings::jni::{jboolean, jint, jmethodID, jobject, jstring, jvalue};
use jvmti_bindings::prelude::JniEnv;
use std::sync::{Mutex, OnceLock};

type Env<'a> = &'a JniEnv;

fn jv(o: jobject) -> jvalue {
    jvalue { l: o }
}

fn jz(v: bool) -> jvalue {
    jvalue { z: v as jboolean }
}

fn ji(v: i32) -> jvalue {
    jvalue { i: v as jint }
}

fn jobj(o: jobject) -> Option<jobject> {
    if o.is_null() {
        None
    } else {
        Some(o)
    }
}

fn clear_if_pending(env: Env) {
    if env.exception_check() {
        env.exception_clear();
    }
}

fn jstr(env: Env, o: jobject) -> Option<String> {
    jobj(o).and_then(|o| env.get_string_utf(o as jstring))
}

// ---------------------------------------------------------------------------
// Static surface: resolved once (see resolve()).
// ---------------------------------------------------------------------------

pub struct Ids {
    bukkit_cls: jobject,
    m_get_server: jmethodID,
    m_server_players: jmethodID,
    m_server_worlds: jmethodID,
    m_iterator: jmethodID,
    m_has_next: jmethodID,
    m_next: jmethodID,
    m_list_size: jmethodID,
    m_list_get: jmethodID,
    uuid_cls: jobject,
    m_uuid_to_string: jmethodID,
    m_uuid_from_string: jmethodID,
    /// Exact adventure Key class from CraftServer's loader.
    key_cls: jobject,
    m_key_key: jmethodID,
    m_pe_add: jmethodID,
    m_pe_remove: jmethodID,
}

static IDS: OnceLock<Option<SendIds>> = OnceLock::new();

/// JNI IDs are raw pointers: usable only from the main thread they were
/// resolved on, but the static needs Send+Sync to exist. Every bridge
/// function is documented main-thread-only.
struct SendIds(Ids);
unsafe impl Send for SendIds {}
unsafe impl Sync for SendIds {}

fn ids() -> Option<&'static Ids> {
    IDS.get().and_then(|o| o.as_ref()).map(|s| &s.0)
}

// ---------------------------------------------------------------------------
// Lazy surfaces: concrete-class method IDs resolved on first live object.
// All main-thread-only; Mutex<Option<_>> allows cheap retry until available.
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct WorldIds {
    m_name: jmethodID,
    m_highest_block: jmethodID,
    m_get_block_at: jmethodID,
    m_chunk_loaded: jmethodID,
    m_get_chunk: jmethodID,
}
// Raw method-ID pointers cached process-lifetime; consumed main-thread-only.
unsafe impl Send for WorldIds {}
static WORLD_IDS: Mutex<Option<WorldIds>> = Mutex::new(None);

#[derive(Clone, Copy)]
struct PlayerIds {
    m_get_unique_id: jmethodID,
    m_get_location: jmethodID,
    m_post_effects: jmethodID,
    m_set_resource_pack: jmethodID,
}
// Raw method-ID pointers cached process-lifetime; consumed main-thread-only.
unsafe impl Send for PlayerIds {}
static PLAYER_IDS: Mutex<Option<PlayerIds>> = Mutex::new(None);

#[derive(Clone, Copy)]
struct LocationIds {
    m_get_block_x: jmethodID,
    m_get_block_y: jmethodID,
    m_get_block_z: jmethodID,
    m_loc_get_world: jmethodID,
}
// Raw method-ID pointers cached process-lifetime; consumed main-thread-only.
unsafe impl Send for LocationIds {}
static LOCATION_IDS: Mutex<Option<LocationIds>> = Mutex::new(None);

#[derive(Clone, Copy)]
struct BlockIds {
    m_is_liquid: jmethodID,
}
// Raw method-ID pointers cached process-lifetime; consumed main-thread-only.
unsafe impl Send for BlockIds {}
static BLOCK_IDS: Mutex<Option<BlockIds>> = Mutex::new(None);

fn method_or_log(env: Env, cls: jobject, name: &str, sig: &str) -> Option<jmethodID> {
    // The FIRST GetMethodID on a freshly-seen class can fail spuriously
    // under this JVMTI agent (NULL result, no pending exception) while the
    // immediate retry binds — a one-shot class-priming effect observed live.
    // Never give up on the first NULL; retry once before reporting failure.
    if let Some(id) = env.get_method_id(cls, name, sig) {
        clear_if_pending(env);
        return Some(id);
    }
    env.exception_clear();
    match env.get_method_id(cls, name, sig) {
        Some(id) => {
            clear_if_pending(env);
            Some(id)
        }
        None => {
            eprintln!("[c-LoD] method resolve FAILED: {name} {sig}");
            if env.exception_check() {
                env.exception_describe();
                env.exception_clear();
            }
            None
        }
    }
}

fn world_ids(env: Env, world: jobject) -> Option<WorldIds> {
    if let Some(w) = WORLD_IDS.lock().unwrap().as_ref() {
        return Some(*w);
    }
    let cls_local = env.get_object_class(world);
    let cls = {
        let g = env.new_global_ref(cls_local);
        g as jobject
    };
    let ids = WorldIds {
        m_name: bind_method(env, cls_local, cls, "getName", "()Ljava/lang/String;")?,
        m_highest_block: bind_method(env, cls_local, cls, "getHighestBlockYAt", "(II)I")?,
        m_chunk_loaded: bind_method(env, cls_local, cls, "isChunkLoaded", "(II)Z")?,
        m_get_chunk: bind_method(env, cls_local, cls, "getChunkAt", "(II)Lorg/bukkit/Chunk;")?,
        m_get_block_at: bind_method(
            env,
            cls_local,
            cls,
            "getBlockAt",
            "(III)Lorg/bukkit/block/Block;",
        )?,
    };
    *WORLD_IDS.lock().unwrap() = Some(ids);
    Some(ids)
}

fn player_ids(env: Env, player: jobject) -> Option<PlayerIds> {
    if let Some(p) = PLAYER_IDS.lock().unwrap().as_ref() {
        return Some(*p);
    }
    let cls_local = env.get_object_class(player);
    let cls = {
        let g = env.new_global_ref(cls_local);
        g as jobject
    };
    let ids = PlayerIds {
        m_get_unique_id: bind_method(env, cls_local, cls, "getUniqueId", "()Ljava/util/UUID;")?,
        m_get_location: bind_method(
            env,
            cls_local,
            cls,
            "getLocation",
            "()Lorg/bukkit/Location;",
        )?,
        m_post_effects: bind_method(
            env,
            cls_local,
            cls,
            "postEffects",
            "()Lio/papermc/paper/entity/PlayerPostEffects;",
        )?,
        m_set_resource_pack: bind_method(
            env,
            cls_local,
            cls,
            "setResourcePack",
            "(Ljava/util/UUID;Ljava/lang/String;[BLjava/lang/String;Z)V",
        )?,
    };
    *PLAYER_IDS.lock().unwrap() = Some(ids);
    Some(ids)
}

fn location_ids(env: Env, loc: jobject) -> Option<LocationIds> {
    if let Some(l) = LOCATION_IDS.lock().unwrap().as_ref() {
        return Some(*l);
    }
    let cls_local = env.get_object_class(loc);
    let cls = {
        let g = env.new_global_ref(cls_local);
        g as jobject
    };
    let ids = LocationIds {
        m_get_block_x: bind_method(env, cls_local, cls, "getBlockX", "()I")?,
        m_get_block_y: bind_method(env, cls_local, cls, "getBlockY", "()I")?,
        m_get_block_z: bind_method(env, cls_local, cls, "getBlockZ", "()I")?,
        m_loc_get_world: bind_method(env, cls_local, cls, "getWorld", "()Lorg/bukkit/World;")?,
    };
    *LOCATION_IDS.lock().unwrap() = Some(ids);
    Some(ids)
}

fn block_ids(env: Env, block: jobject) -> Option<BlockIds> {
    if let Some(b) = BLOCK_IDS.lock().unwrap().as_ref() {
        return Some(*b);
    }
    let cls_local = env.get_object_class(block);
    let cls = {
        let g = env.new_global_ref(cls_local);
        g as jobject
    };
    let ids = BlockIds {
        m_is_liquid: bind_method(env, cls_local, cls, "isLiquid", "()Z")?,
    };
    *BLOCK_IDS.lock().unwrap() = Some(ids);
    Some(ids)
}

// ---------------------------------------------------------------------------
// One-time resolve of the static surface.
// ---------------------------------------------------------------------------

/// Bind a virtual method through the probe-then-globalize sequence that
/// works under this agent (see resolve(): local-ref GetMethodID fails
/// silently until the class was forced to link by a local probe).
fn bind_method(
    env: Env,
    local_cls: jobject,
    global_cls: jobject,
    name: &str,
    sig: &str,
) -> Option<jmethodID> {
    let _ = env.get_method_id(local_cls, name, sig); // probe: may fail, primes the class
    env.exception_clear();
    match env.get_method_id(global_cls, name, sig) {
        Some(id) => {
            clear_if_pending(env);
            Some(id)
        }
        None => {
            eprintln!("[c-LoD] method resolve FAILED: {name} {sig}");
            if env.exception_check() {
                env.exception_describe();
                env.exception_clear();
            }
            None
        }
    }
}

fn resolve(env: Env) -> Option<Ids> {
    use cplug_sdk::classes as sdkcls;

    // JVMTI scan (loader-independent) — ONLY for single-copy bootstrap
    // classes and Bukkit. Anything Paper-API-specific uses live objects.
    macro_rules! cls {
        ($name:expr) => {{
            match sdkcls::find_class($name) {
                Some(c) => {
                    clear_if_pending(env);
                    c.as_jclass()
                }
                None => {
                    eprintln!("[c-LoD] class resolve FAILED: {}", $name);
                    return None;
                }
            }
        }};
    }
    macro_rules! m {
        ($c:expr, $n:expr, $s:expr) => {
            method_or_log(env, $c, $n, $s)?
        };
    }
    macro_rules! sm {
        ($c:expr, $n:expr, $s:expr) => {
            match env.get_static_method_id($c, $n, $s) {
                Some(id) => {
                    clear_if_pending(env);
                    id
                }
                None => {
                    eprintln!("[c-LoD] static method resolve FAILED: {} {}", $n, $s);
                    if env.exception_check() {
                        env.exception_describe();
                        env.exception_clear();
                    }
                    return None;
                }
            }
        };
    }

    // Bootstrap classes live in a single stable copy — the JVMTI scan is
    // safe for them. EVERYTHING from paper-api is resolved through
    // Class.forName in CraftServer's own loader instead: the scan can grab
    // an API class mid-redefinition (Paper remaps API classes during boot)
    // and cache a stale global ref whose method table mismatches forever
    // (GetMethodID silently returns NULL for every method).
    let collection_cls = cls!("java/util/Collection");
    let iterator_cls = cls!("java/util/Iterator");
    let list_cls = cls!("java/util/List");
    let uuid_cls = cls!("java/util/UUID");
    let class_cls = cls!("java/lang/Class");

    let m_for_name = sm!(
        class_cls,
        "forName",
        "(Ljava/lang/String;ZLjava/lang/ClassLoader;)Ljava/lang/Class;"
    );
    let m_iterator = m!(collection_cls, "iterator", "()Ljava/util/Iterator;");
    let m_has_next = m!(iterator_cls, "hasNext", "()Z");
    let m_next = m!(iterator_cls, "next", "()Ljava/lang/Object;");
    let m_list_size = m!(list_cls, "size", "()I");
    let m_list_get = m!(list_cls, "get", "(I)Ljava/lang/Object;");
    let m_uuid_to_string = m!(uuid_cls, "toString", "()Ljava/lang/String;");
    let m_uuid_from_string = sm!(
        uuid_cls,
        "fromString",
        "(Ljava/lang/String;)Ljava/util/UUID;"
    );

    // Probe the live server through the scanned Bukkit (bootstrap-adjacent,
    // verified working: getServer is its only use) — the probe object is
    // then the ground truth for CraftServer's loader.
    let bukkit_probe = cls!("org/bukkit/Bukkit");
    let m_get_server_probe = sm!(bukkit_probe, "getServer", "()Lorg/bukkit/Server;");
    let server_obj = jobj(env.call_static_object_method(bukkit_probe, m_get_server_probe, &[]));
    clear_if_pending(env);
    let server_obj = server_obj?;

    // Live server -> concrete CraftServer class -> server-surface methods.
    let server_cls = env.get_object_class(server_obj);

    // Force-load + initialize the API classes in CraftServer's loader and
    // keep the EXACT Class objects (no scan ambiguity, no stale cache):
    // server.getClass().getClassLoader() -> Class.forName(name, true, cl).
    let m_get_class = m!(class_cls, "getClass", "()Ljava/lang/Class;");
    let m_get_class_loader = m!(class_cls, "getClassLoader", "()Ljava/lang/ClassLoader;");
    let k = jobj(env.call_object_method(server_obj, m_get_class, &[]));
    let loader = k.and_then(|k| jobj(env.call_object_method(k, m_get_class_loader, &[])));
    let loader = loader?;
    let force_load = |dotted: &str| -> Option<jobject> {
        let jn = env.new_string_utf(dotted)?;
        let k =
            env.call_static_object_method(class_cls, m_for_name, &[jv(jn), jz(true), jv(loader)]);
        clear_if_pending(env);
        jobj(k)
    };

    let bukkit_cls = force_load("org.bukkit.Bukkit")?;
    let key_cls = force_load("net.kyori.adventure.key.Key")?;
    let post_effects_cls = force_load("io.papermc.paper.entity.PlayerPostEffects")?;
    // getServer re-resolved on the EXACT Bukkit class from this loader.
    let m_get_server = sm!(bukkit_cls, "getServer", "()Lorg/bukkit/Server;");

    // JNI quirk discovered live under this JVMTI agent: GetMethodID on a
    // LOCAL-ref jclass returns NULL (no pending exception); the SAME class
    // through a GLOBAL ref binds — but only after a failed local-ref attempt
    // forced the class to finish linking (probe-then-globalize,
    // see bind_method). Used for every Paper-API method below.
    let server_cls_local = server_cls;
    let server_cls = {
        let g = env.new_global_ref(server_cls);
        g as jobject
    };

    let m_server_players = bind_method(
        env,
        server_cls_local,
        server_cls,
        "getOnlinePlayers",
        "()Ljava/util/Collection;",
    )?;
    let m_server_worlds = bind_method(
        env,
        server_cls_local,
        server_cls,
        "getWorlds",
        "()Ljava/util/List;",
    )?;
    #[allow(clippy::used_underscore_binding)]
    let m_key_key = sm!(
        key_cls,
        "key",
        "(Ljava/lang/String;)Lnet/kyori/adventure/key/Key;"
    );
    let m_pe_add = m!(post_effects_cls, "add", "(Lnet/kyori/adventure/key/Key;)Z");
    let m_pe_remove = m!(
        post_effects_cls,
        "remove",
        "(Lnet/kyori/adventure/key/Key;)Z"
    );

    Some(Ids {
        bukkit_cls,
        m_get_server,
        m_server_players,
        m_server_worlds,
        m_iterator,
        m_has_next,
        m_next,
        m_list_size,
        m_list_get,
        uuid_cls,
        m_uuid_to_string,
        m_uuid_from_string,
        key_cls,
        m_key_key,
        m_pe_add,
        m_pe_remove,
    })
}

/// Resolve IDs. Call from the main thread each tick until it returns true.
///
/// The static surface requires a live server object (CraftServer methods),
/// which only exists after Bukkit.setServer — before that, retry next tick.
pub fn warmup(env: Env) -> bool {
    if let Some(s) = IDS.get() {
        return s.is_some();
    }
    match resolve(env) {
        Some(ids) => {
            let _ = IDS.set(Some(SendIds(ids)));
            true
        }
        None => {
            clear_if_pending(env);
            false
        }
    }
}

// ---------------------------------------------------------------------------
// Data returned to the engine.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct PlayerInfo {
    pub uuid: String,
    pub world: String,
    pub bx: i32,
    #[allow(dead_code)] // y is useful for future band filtering
    pub by: i32,
    pub bz: i32,
}

pub struct WorldRef {
    pub name: String,
    pub obj: jobject,
}

// ---------------------------------------------------------------------------
// Bridge functions (main thread, attached env).
// ---------------------------------------------------------------------------

fn server(env: Env, ids: &Ids) -> Option<jobject> {
    jobj(env.call_static_object_method(ids.bukkit_cls, ids.m_get_server, &[]))
}

fn online_players_iter(env: Env, ids: &Ids) -> Option<jobject> {
    let srv = server(env, ids)?;
    let collection = jobj(env.call_object_method(srv, ids.m_server_players, &[]))?;
    let iterator = jobj(env.call_object_method(collection, ids.m_iterator, &[]))?;
    Some(iterator)
}

pub fn players(env: Env) -> Vec<PlayerInfo> {
    let Some(ids) = ids() else { return Vec::new() };
    let Some(iterator) = online_players_iter(env, ids) else {
        clear_if_pending(env);
        return Vec::new();
    };

    let mut out = Vec::new();
    loop {
        if !env.call_boolean_method(iterator, ids.m_has_next, &[]) {
            break;
        }
        let Some(player) = jobj(env.call_object_method(iterator, ids.m_next, &[])) else {
            break;
        };
        let Some(pids) = player_ids(env, player) else {
            clear_if_pending(env);
            continue;
        };

        let Some(uuid) = jobj(env.call_object_method(player, pids.m_get_unique_id, &[]))
            .and_then(|u| jobj(env.call_object_method(u, ids.m_uuid_to_string, &[])))
            .and_then(|s| jstr(env, s))
        else {
            clear_if_pending(env);
            continue;
        };

        let Some(loc) = jobj(env.call_object_method(player, pids.m_get_location, &[])) else {
            clear_if_pending(env);
            continue;
        };
        let Some(lids) = location_ids(env, loc) else {
            clear_if_pending(env);
            continue;
        };
        let bx = env.call_int_method(loc, lids.m_get_block_x, &[]);
        let by = env.call_int_method(loc, lids.m_get_block_y, &[]);
        let bz = env.call_int_method(loc, lids.m_get_block_z, &[]);
        let world = jobj(env.call_object_method(loc, lids.m_loc_get_world, &[]))
            .and_then(|w| {
                let wid = world_ids(env, w)?;
                jobj(env.call_object_method(w, wid.m_name, &[]))
            })
            .and_then(|s| jstr(env, s))
            .unwrap_or_default();
        clear_if_pending(env);

        out.push(PlayerInfo {
            uuid,
            world,
            bx,
            by,
            bz,
        });
    }
    clear_if_pending(env);
    out
}

/// World handle by name (first world when `name` is empty).
pub fn find_world(env: Env, name: &str) -> Option<WorldRef> {
    let ids = ids()?;
    let srv = server(env, ids)?;
    let list = jobj(env.call_object_method(srv, ids.m_server_worlds, &[]))?;
    let n = env.call_int_method(list, ids.m_list_size, &[]);
    for i in 0..n {
        let Some(world) = jobj(env.call_object_method(list, ids.m_list_get, &[ji(i)])) else {
            clear_if_pending(env);
            continue;
        };
        let Some(wid) = world_ids(env, world) else {
            clear_if_pending(env);
            continue;
        };
        let Some(wname) =
            jobj(env.call_object_method(world, wid.m_name, &[])).and_then(|s| jstr(env, s))
        else {
            clear_if_pending(env);
            continue;
        };
        if name.is_empty() || wname == name {
            return Some(WorldRef {
                name: wname,
                obj: world,
            });
        }
    }
    clear_if_pending(env);
    None
}

/// Force-load (and, if needed, generate) one chunk. Used once at boot to
/// warm the core region for the static snapshot.
pub fn warm_chunk(env: Env, world: jobject, cx: i32, cz: i32) -> bool {
    let Some(wid) = world_ids(env, world) else {
        clear_if_pending(env);
        return false;
    };
    let _ = env.call_object_method(world, wid.m_get_chunk, &[ji(cx), ji(cz)]);
    let ok = !env.exception_check();
    clear_if_pending(env);
    ok
}

/// Sample the surface height at one column. Returns (blockY, isLiquid).
pub fn sample_height(env: Env, world: jobject, x: i32, z: i32) -> Option<(i32, bool)> {
    let wid = world_ids(env, world)?;
    if env.exception_check() {
        env.exception_clear();
        return None;
    }
    // Only sample loaded chunks: getHighestBlockYAt would otherwise force
    // synchronous chunk generation and stall the main thread for minutes.
    // Unloaded cells stay invalid and get picked up by later sweeps (the
    // scan task wraps around the atlas) once players load those chunks.
    let loaded = env.call_boolean_method(world, wid.m_chunk_loaded, &[ji(x >> 4), ji(z >> 4)]);
    if env.exception_check() {
        env.exception_clear();
        return None;
    }
    if !loaded {
        return None;
    }
    let y = env.call_int_method(world, wid.m_highest_block, &[ji(x), ji(z)]);
    if env.exception_check() {
        env.exception_clear();
        return None;
    }
    if y < -64 {
        return Some((y, false));
    }
    let Some(block) =
        jobj(env.call_object_method(world, wid.m_get_block_at, &[ji(x), ji(y), ji(z)]))
    else {
        clear_if_pending(env);
        return Some((y, false));
    };
    let Some(bid) = block_ids(env, block) else {
        clear_if_pending(env);
        return Some((y, false));
    };
    let liquid = env.call_boolean_method(block, bid.m_is_liquid, &[]);
    clear_if_pending(env);
    Some((y, liquid))
}

// ---------------------------------------------------------------------------
// Actions: resource pack + post effects.
// ---------------------------------------------------------------------------

pub fn parse_uuid(env: Env, s: &str) -> Option<jobject> {
    let ids = ids()?;
    let j = env.new_string_utf(s)?;
    let u = env.call_static_object_method(ids.uuid_cls, ids.m_uuid_from_string, &[jv(j)]);
    clear_if_pending(env);
    jobj(u)
}

pub fn make_key(env: Env, id: &str) -> Option<jobject> {
    let ids = ids()?;
    let jid = env.new_string_utf(id)?;
    let k = env.call_static_object_method(ids.key_cls, ids.m_key_key, &[jv(jid)]);
    clear_if_pending(env);
    jobj(k)
}

/// Player object lookup by uuid string (main thread).
pub fn player_by_uuid(env: Env, uuid: &str) -> Option<jobject> {
    let ids = ids()?;
    let Some(iterator) = online_players_iter(env, ids) else {
        clear_if_pending(env);
        return None;
    };
    while env.call_boolean_method(iterator, ids.m_has_next, &[]) {
        let Some(player) = jobj(env.call_object_method(iterator, ids.m_next, &[])) else {
            break;
        };
        let Some(pids) = player_ids(env, player) else {
            clear_if_pending(env);
            continue;
        };
        let Some(puuid) = jobj(env.call_object_method(player, pids.m_get_unique_id, &[]))
            .and_then(|u| jobj(env.call_object_method(u, ids.m_uuid_to_string, &[])))
            .and_then(|s| jstr(env, s))
        else {
            clear_if_pending(env);
            continue;
        };
        if puuid == uuid {
            return Some(player);
        }
    }
    clear_if_pending(env);
    None
}

pub fn push_pack(
    env: Env,
    player: jobject,
    pack_uuid: jobject,
    url: &str,
    sha1: &[u8; 20],
    prompt: &str,
    force: bool,
) -> bool {
    if ids().is_none() {
        return false; // bridge not resolved yet
    }
    let Some(pids) = player_ids(env, player) else {
        clear_if_pending(env);
        return false;
    };
    let Some(jurl) = env.new_string_utf(url) else {
        return false;
    };
    let Some(jprompt) = env.new_string_utf(prompt) else {
        return false;
    };
    let Some(jarr) = env.new_byte_array(20) else {
        return false;
    };
    let bytes: Vec<jvmti_bindings::jni::jbyte> = sha1.iter().map(|b| *b as i8).collect();
    env.set_byte_array_region(jarr, 0, 20, &bytes);

    let args = [jv(pack_uuid), jv(jurl), jv(jarr), jv(jprompt), jz(force)];
    env.call_void_method(player, pids.m_set_resource_pack, &args);
    let ok = !env.exception_check();
    clear_if_pending(env);
    ok
}

pub fn add_effect(env: Env, player: jobject, key: jobject) -> bool {
    let Some(ids) = ids() else { return false };
    let Some(pids) = player_ids(env, player) else {
        clear_if_pending(env);
        return false;
    };
    let Some(pe) = jobj(env.call_object_method(player, pids.m_post_effects, &[])) else {
        clear_if_pending(env);
        return false;
    };
    let ok = env.call_boolean_method(pe, ids.m_pe_add, &[jv(key)]);
    clear_if_pending(env);
    ok
}

pub fn remove_effect(env: Env, player: jobject, key: jobject) -> bool {
    let Some(ids) = ids() else { return false };
    let Some(pids) = player_ids(env, player) else {
        clear_if_pending(env);
        return false;
    };
    let Some(pe) = jobj(env.call_object_method(player, pids.m_post_effects, &[])) else {
        clear_if_pending(env);
        return false;
    };
    let ok = env.call_boolean_method(pe, ids.m_pe_remove, &[jv(key)]);
    clear_if_pending(env);
    ok
}
