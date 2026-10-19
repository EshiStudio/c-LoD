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

use jvmti_bindings::jni::{jboolean, jint, jmethodID, jobject, jstring, jvalue};
use jvmti_bindings::prelude::JniEnv;
use std::sync::OnceLock;

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

struct Ids {
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
    m_get_unique_id: jmethodID,
    m_get_location: jmethodID,
    m_set_resource_pack: jmethodID,
    m_post_effects: jmethodID,
    m_get_block_x: jmethodID,
    m_get_block_y: jmethodID,
    m_get_block_z: jmethodID,
    m_loc_get_world: jmethodID,
    m_world_name: jmethodID,
    m_highest_block: jmethodID,
    m_get_block_at: jmethodID,
    m_is_liquid: jmethodID,
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

/// Resolve all classes/method IDs once. Returns None on any miss; bridge
/// functions then no-op instead of crashing the JVM on a null ID.
fn resolve(env: Env) -> Option<Ids> {
    macro_rules! cls {
        ($name:expr) => {
            env.find_class($name)?
        };
    }
    macro_rules! m {
        ($c:expr, $n:expr, $s:expr) => {
            env.get_method_id($c, $n, $s)?
        };
    }
    macro_rules! sm {
        ($c:expr, $n:expr, $s:expr) => {
            env.get_static_method_id($c, $n, $s)?
        };
    }

    let bukkit_cls = cls!("org/bukkit/Bukkit");
    let server_cls = cls!("org/bukkit/Server");
    let collection_cls = cls!("java/util/Collection");
    let iterator_cls = cls!("java/util/Iterator");
    let list_cls = cls!("java/util/List");
    let uuid_cls = cls!("java/util/UUID");
    let player_cls = cls!("org/bukkit/entity/Player");
    let location_cls = cls!("org/bukkit/Location");
    let world_cls = cls!("org/bukkit/World");
    let block_cls = cls!("org/bukkit/block/Block");
    let key_cls = cls!("net/kyori/adventure/key/Key");
    let post_effects_cls = cls!("io/papermc/paper/entity/PlayerPostEffects");

    let m_get_server = sm!(bukkit_cls, "getServer", "()Lorg/bukkit/Server;");
    let m_server_players = m!(server_cls, "getOnlinePlayers", "()Ljava/lang/Collection;");
    let m_server_worlds = m!(server_cls, "getWorlds", "()Ljava/util/List;");
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
    let m_get_unique_id = m!(player_cls, "getUniqueId", "()Ljava/util/UUID;");
    let m_get_location = m!(player_cls, "getLocation", "()Lorg/bukkit/Location;");
    let m_set_resource_pack = m!(
        player_cls,
        "setResourcePack",
        "(Ljava/util/UUID;Ljava/lang/String;[BLjava/lang/String;Z)V"
    );
    let m_post_effects = m!(
        player_cls,
        "postEffects",
        "()Lio/papermc/paper/entity/PlayerPostEffects;"
    );
    let m_get_block_x = m!(location_cls, "getBlockX", "()I");
    let m_get_block_y = m!(location_cls, "getBlockY", "()I");
    let m_get_block_z = m!(location_cls, "getBlockZ", "()I");
    let m_loc_get_world = m!(location_cls, "getWorld", "()Lorg/bukkit/World;");
    let m_world_name = m!(world_cls, "getName", "()Ljava/lang/String;");
    let m_highest_block = m!(world_cls, "getHighestBlockYAt", "(II)I");
    let m_get_block_at = m!(world_cls, "getBlockAt", "(III)Lorg/bukkit/block/Block;");
    let m_is_liquid = m!(block_cls, "isLiquid", "()Z");
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
        m_get_unique_id,
        m_get_location,
        m_set_resource_pack,
        m_post_effects,
        m_get_block_x,
        m_get_block_y,
        m_get_block_z,
        m_loc_get_world,
        m_world_name,
        m_highest_block,
        m_get_block_at,
        m_is_liquid,
        key_cls,
        m_key_key,
        m_pe_add,
        m_pe_remove,
    })
}

/// Resolve IDs (idempotent). Call once on the main thread during boot.
pub fn warmup(env: Env) -> bool {
    IDS.get_or_init(|| resolve(env).map(SendIds)).is_some()
}

fn ids() -> Option<&'static Ids> {
    IDS.get().and_then(|o| o.as_ref()).map(|s| &s.0)
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

        let Some(uuid) = jobj(env.call_object_method(player, ids.m_get_unique_id, &[]))
            .and_then(|u| jobj(env.call_object_method(u, ids.m_uuid_to_string, &[])))
            .and_then(|s| jstr(env, s))
        else {
            clear_if_pending(env);
            continue;
        };

        let Some(loc) = jobj(env.call_object_method(player, ids.m_get_location, &[])) else {
            clear_if_pending(env);
            continue;
        };
        let bx = env.call_int_method(loc, ids.m_get_block_x, &[]);
        let by = env.call_int_method(loc, ids.m_get_block_y, &[]);
        let bz = env.call_int_method(loc, ids.m_get_block_z, &[]);
        let world = jobj(env.call_object_method(loc, ids.m_loc_get_world, &[]))
            .and_then(|w| jstr(env, name_of(env, ids, w)))
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

fn name_of(env: Env, ids: &Ids, world: jobject) -> jobject {
    env.call_object_method(world, ids.m_world_name, &[])
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
        let Some(wname) = jobj(name_of(env, ids, world)).and_then(|s| jstr(env, s)) else {
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
    None
}

/// Sample the surface height at one column. Returns (blockY, isLiquid).
pub fn sample_height(env: Env, world: jobject, x: i32, z: i32) -> Option<(i32, bool)> {
    let ids = ids()?;
    if env.exception_check() {
        env.exception_clear();
        return None;
    }
    let y = env.call_int_method(world, ids.m_highest_block, &[ji(x), ji(z)]);
    if env.exception_check() {
        env.exception_clear();
        return None;
    }
    if y < -64 {
        return Some((y, false));
    }
    let Some(block) =
        jobj(env.call_object_method(world, ids.m_get_block_at, &[ji(x), ji(y), ji(z)]))
    else {
        clear_if_pending(env);
        return Some((y, false));
    };
    let liquid = env.call_boolean_method(block, ids.m_is_liquid, &[]);
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
        let Some(puuid) = jobj(env.call_object_method(player, ids.m_get_unique_id, &[]))
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
    let Some(ids) = ids() else { return false };
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
    env.call_void_method(player, ids.m_set_resource_pack, &args);
    let ok = !env.exception_check();
    clear_if_pending(env);
    ok
}

pub fn add_effect(env: Env, player: jobject, key: jobject) -> bool {
    let Some(ids) = ids() else { return false };
    let Some(pe) = jobj(env.call_object_method(player, ids.m_post_effects, &[])) else {
        clear_if_pending(env);
        return false;
    };
    let ok = env.call_boolean_method(pe, ids.m_pe_add, &[jv(key)]);
    clear_if_pending(env);
    ok
}

pub fn remove_effect(env: Env, player: jobject, key: jobject) -> bool {
    let Some(ids) = ids() else { return false };
    let Some(pe) = jobj(env.call_object_method(player, ids.m_post_effects, &[])) else {
        clear_if_pending(env);
        return false;
    };
    let ok = env.call_boolean_method(pe, ids.m_pe_remove, &[jv(key)]);
    clear_if_pending(env);
    ok
}
