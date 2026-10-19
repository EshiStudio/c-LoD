//! Bukkit API facade: one-time method-id bootstrap + read ops.
//!
//! Every JVM interaction goes through cached `jmethodID`s resolved once on
//! the main thread after the kernel is up. Resolution failures produce
//! precise diagnostics and disable the engine (no guessing at runtime).

use crate::jvm::jniutil::*;
use crate::state::msg::PlayerSnapshot;
use cplug_sdk::classes::{find_class, ClassRef};
use cplug_sdk::jni_util::with_attached;
use jvmti_bindings::prelude::*;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

pub(crate) struct BukkitApi {
    // classes actively used post-bootstrap (process-lifetime global refs)
    pub bukkit: ClassRef,
    pub location: ClassRef,
    pub material: ClassRef,
    pub entity_type: ClassRef,
    pub brightness: ClassRef,
    pub uuid: ClassRef,
    pub matrix: ClassRef,

    // the live Server instance (global ref) — Server methods are instance
    pub server_obj: jni::jobject,

    // static entry points
    pub m_online_players: jni::jmethodID,
    // instance entry points (resolved on the Server class, called on server_obj)
    pub m_view_distance: jni::jmethodID,
    pub m_world_by_name: jni::jmethodID,
    pub m_get_entity: jni::jmethodID,
    pub m_uuid_from_string: jni::jmethodID,
    pub m_material_value_of: jni::jmethodID,
    pub f_block_display_type: jni::jfieldID,

    // instance methods
    pub m_to_array: jni::jmethodID,
    pub m_world_name: jni::jmethodID,
    pub m_world_chunk_at: jni::jmethodID,
    pub m_world_spawn: jni::jmethodID,
    pub m_world_forceload: jni::jmethodID,
    pub m_chunk_snap: jni::jmethodID,
    pub m_snap_top_y: jni::jmethodID,
    pub m_snap_block_type: jni::jmethodID,
    pub m_loc_world: jni::jmethodID,
    pub m_loc_new: jni::jmethodID,
    pub m_loc_bx: jni::jmethodID,
    pub m_loc_bz: jni::jmethodID,
    pub m_entity_location: jni::jmethodID,
    pub m_entity_uuid: jni::jmethodID,
    pub m_entity_remove: jni::jmethodID,
    pub m_entity_dead: jni::jmethodID,
    pub m_entity_persistent: jni::jmethodID,
    pub m_entity_invulnerable: Option<jni::jmethodID>,
    pub m_world_min_height: Option<jni::jmethodID>,
    pub m_material_name: jni::jmethodID,
    pub m_material_ordinal: jni::jmethodID,
    pub m_material_block_data: jni::jmethodID,
    pub m_uuid_to_string: jni::jmethodID,
    pub m_display_matrix: jni::jmethodID,
    pub m_display_interp_dur: jni::jmethodID,
    pub m_display_interp_delay: jni::jmethodID,
    pub m_display_teleport_dur: jni::jmethodID,
    pub m_display_brightness: jni::jmethodID,
    pub m_block_display_set_block: jni::jmethodID,
    pub m_brightness_new: jni::jmethodID,
    pub m_matrix_new: jni::jmethodID,
    pub m_matrix_scale: jni::jmethodID,

    // runtime caches
    pub material_refs: Mutex<HashMap<String, jni::jobject>>,
    pub ordinal_colors: Mutex<HashMap<i32, [u8; 3]>>,
    pub world_refs: Mutex<HashMap<String, jni::jobject>>,
}

static API: OnceLock<Arc<BukkitApi>> = OnceLock::new();

// SAFETY: BukkitApi holds only JVM-wide handles (global ClassRefs,
// jmethodID/jfieldID) valid from any attached thread per the JNI spec.
unsafe impl Send for BukkitApi {}
unsafe impl Sync for BukkitApi {}

pub fn api() -> Option<&'static Arc<BukkitApi>> {
    API.get()
}

macro_rules! need {
    ($e:expr, $what:expr) => {
        match $e {
            Ok(v) => v,
            Err(e) => return Some(Err(format!("{}: {}", $what, e))),
        }
    };
}

macro_rules! opt {
    ($env:expr, $cls:expr, $name:expr, $sig:expr) => {
        $env.get_method_id($cls, $name, $sig)
    };
}

/// Resolve every class/method once. Must run on the main thread (kernel up).
/// Idempotent; logs the resolved surface on first success.
pub fn bootstrap() -> Result<(), String> {
    if API.get().is_some() {
        return Ok(());
    }
    let out = with_attached(bootstrap_env).flatten();
    match out {
        Some(Ok(api)) => {
            let _ = API.set(Arc::new(api));
            Ok(())
        }
        Some(Err(e)) => Err(e),
        None => Err("JNI attach failed during bootstrap".into()),
    }
}

/// Kernel classes load lazily (org/bukkit/ChunkSnapshot etc. only appear on
/// first use) — `find_class` alone is not enough. `cls` falls back to the
/// kernel's own class loader (anchor: org/bukkit/Bukkit), forcing a load.
pub(crate) fn load_class(env: &JniEnv, name: &str) -> Result<ClassRef, String> {
    if let Some(c) = find_class(name) {
        return Ok(c);
    }
    let loader = kernel_loader(env)?;
    let dotted = name.replace('/', ".");
    let s = new_jstr(env, &dotted).ok_or("oom: class name")?;
    let class_cls = env.find_class("java/lang/Class").ok_or_else(|| {
        swallow(env, "find java/lang/Class");
        "class not loaded: java/lang/Class".to_string()
    })?;
    let load_mid = static_method(
        env,
        class_cls,
        "forName",
        "(Ljava/lang/String;ZLjava/lang/ClassLoader;)Ljava/lang/Class;",
    )?;
    // initialize=true: GetMethodID on a merely linked (uninitialized) class
    // derefs null InstanceKlass fields and SIGSEGVs — the SDK guards its own
    // lookups on JVMTI_CLASS_STATUS_INITIALIZED for the same reason.
    let loaded = call_static_obj(
        env,
        class_cls,
        load_mid,
        &[jv_o(s), jv_z(true), jv_o(loader)],
        &format!("Class.forName({name})"),
    )
    .ok_or_else(|| format!("class not loadable: {name}"))?;
    env.delete_local_ref(s);
    // own a process-lifetime global ref regardless of which path returns
    let g = env.new_global_ref(loaded);
    env.delete_local_ref(loaded);
    LEAKED_REFS.with(|v| v.borrow_mut().push(g));
    // prefer the SDK cache when it now resolves (it validated the
    // INITIALIZED status); fall back to our own ref
    if let Some(c) = find_class(name) {
        return Ok(c);
    }
    Ok(ClassRef(g))
}

thread_local! {
    /// Global refs created during bootstrap. The bootstrap runs once on the
    /// main thread and the refs live for the process — this thread-local
    /// outlives them (the main thread never dies).
    static LEAKED_REFS: std::cell::RefCell<Vec<jni::jobject>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// The class loader of the kernel (anchored on a bootstrap-loaded class).
pub(crate) fn kernel_loader(env: &JniEnv) -> Result<jni::jobject, String> {
    static LOADER: OnceLock<usize> = OnceLock::new();
    if let Some(l) = LOADER.get() {
        return Ok(*l as jni::jobject);
    }
    let anchor =
        find_class("org/bukkit/Bukkit").ok_or("anchor class org/bukkit/Bukkit not loaded")?;
    let class_cls = env.find_class("java/lang/Class").ok_or_else(|| {
        swallow(env, "find java/lang/Class");
        "class not loaded: java/lang/Class".to_string()
    })?;
    let mid = method(
        env,
        class_cls,
        "getClassLoader",
        "()Ljava/lang/ClassLoader;",
    )?;
    let loader = call_obj(env, anchor.as_jclass(), mid, &[], "Class.getClassLoader")
        .ok_or("kernel class loader not resolvable")?;
    let g = env.new_global_ref(loader);
    env.delete_local_ref(loader);
    LEAKED_REFS.with(|v| v.borrow_mut().push(g));
    let _ = LOADER.set(g as usize);
    Ok(g)
}

fn bootstrap_env(env: &JniEnv) -> Option<Result<BukkitApi, String>> {
    let bukkit = need!(load_class(env, "org/bukkit/Bukkit"), "bukkit");
    let server = need!(load_class(env, "org/bukkit/Server"), "server");
    let world = need!(load_class(env, "org/bukkit/World"), "world");
    let chunk = need!(load_class(env, "org/bukkit/Chunk"), "chunk");
    let chunk_snapshot = need!(
        load_class(env, "org/bukkit/ChunkSnapshot"),
        "chunk_snapshot"
    );
    let location = need!(load_class(env, "org/bukkit/Location"), "location");
    let material = need!(load_class(env, "org/bukkit/Material"), "material");
    let entity = need!(load_class(env, "org/bukkit/entity/Entity"), "entity");
    let player = need!(load_class(env, "org/bukkit/entity/Player"), "player");
    let display = need!(load_class(env, "org/bukkit/entity/Display"), "display");
    let block_display = need!(
        load_class(env, "org/bukkit/entity/BlockDisplay"),
        "block_display"
    );
    let entity_type = need!(
        load_class(env, "org/bukkit/entity/EntityType"),
        "entity_type"
    );
    let brightness = need!(
        load_class(env, "org/bukkit/entity/Display$Brightness"),
        "brightness"
    );
    let uuid = need!(load_class(env, "java/util/UUID"), "uuid");
    let matrix = need!(load_class(env, "org/joml/Matrix4f"), "matrix");

    let (b, s, w, c, cs) = (
        bukkit.as_jclass(),
        server.as_jclass(),
        world.as_jclass(),
        chunk.as_jclass(),
        chunk_snapshot.as_jclass(),
    );
    let (loc, mat, ent, _pl) = (
        location.as_jclass(),
        material.as_jclass(),
        entity.as_jclass(),
        player.as_jclass(),
    );
    let (disp, bd, et, br, uu, mx) = (
        display.as_jclass(),
        block_display.as_jclass(),
        entity_type.as_jclass(),
        brightness.as_jclass(),
        uuid.as_jclass(),
        matrix.as_jclass(),
    );

    // java/util/Collection for toArray
    let collection = need!(load_class(env, "java/util/Collection"), "collection");

    // live Server object (global ref, process lifetime)
    let m_get_server = need!(
        static_method(env, b, "getServer", "()Lorg/bukkit/Server;"),
        "Bukkit"
    );
    let server_local = match call_static_obj(env, b, m_get_server, &[], "getServer") {
        Some(o) => o,
        None => {
            return Some(Err(
                "Bukkit.getServer() returned null — kernel not up yet".into()
            ))
        }
    };
    let server_obj = env.new_global_ref(server_local);
    env.delete_local_ref(server_local);

    let api = BukkitApi {
        server_obj,
        m_online_players: need!(
            static_method(env, b, "getOnlinePlayers", "()Ljava/util/Collection;"),
            "Bukkit"
        ),
        m_view_distance: need!(method(env, s, "getViewDistance", "()I"), "Server"),
        m_world_by_name: need!(
            method(env, s, "getWorld", "(Ljava/lang/String;)Lorg/bukkit/World;"),
            "Server"
        ),
        m_get_entity: need!(
            method(
                env,
                s,
                "getEntity",
                "(Ljava/util/UUID;)Lorg/bukkit/entity/Entity;"
            ),
            "Server"
        ),
        m_uuid_from_string: need!(
            static_method(
                env,
                uu,
                "fromString",
                "(Ljava/lang/String;)Ljava/util/UUID;"
            ),
            "UUID"
        ),
        m_material_value_of: need!(
            static_method(
                env,
                mat,
                "valueOf",
                "(Ljava/lang/String;)Lorg/bukkit/Material;"
            ),
            "Material"
        ),
        f_block_display_type: need!(
            static_field(env, et, "BLOCK_DISPLAY", "Lorg/bukkit/entity/EntityType;"),
            "EntityType"
        ),

        m_to_array: need!(
            method(
                env,
                collection.as_jclass(),
                "toArray",
                "()[Ljava/lang/Object;"
            ),
            "Collection"
        ),
        m_world_name: need!(method(env, w, "getName", "()Ljava/lang/String;"), "World"),
        m_world_chunk_at: need!(
            method(env, w, "getChunkAt", "(II)Lorg/bukkit/Chunk;"),
            "World"
        ),
        m_world_spawn: need!(
            method(
                env,
                w,
                "spawnEntity",
                "(Lorg/bukkit/Location;Lorg/bukkit/entity/EntityType;)Lorg/bukkit/entity/Entity;"
            ),
            "World"
        ),
        m_world_forceload: need!(method(env, w, "setChunkForceLoaded", "(IIZ)V"), "World"),
        m_chunk_snap: need!(
            method(
                env,
                c,
                "getChunkSnapshot",
                "(ZZZ)Lorg/bukkit/ChunkSnapshot;"
            ),
            "Chunk"
        ),
        m_snap_top_y: need!(
            method(env, cs, "getHighestBlockYAt", "(II)I"),
            "ChunkSnapshot"
        ),
        m_snap_block_type: need!(
            method(env, cs, "getBlockType", "(III)Lorg/bukkit/Material;"),
            "ChunkSnapshot"
        ),
        m_loc_world: need!(
            method(env, loc, "getWorld", "()Lorg/bukkit/World;"),
            "Location"
        ),
        m_loc_new: need!(
            method(env, loc, "<init>", "(Lorg/bukkit/World;DDD)V"),
            "Location"
        ),
        m_loc_bx: need!(method(env, loc, "getBlockX", "()I"), "Location"),
        m_loc_bz: need!(method(env, loc, "getBlockZ", "()I"), "Location"),
        m_entity_location: need!(
            method(env, ent, "getLocation", "()Lorg/bukkit/Location;"),
            "Entity"
        ),
        m_entity_uuid: need!(
            method(env, ent, "getUniqueId", "()Ljava/util/UUID;"),
            "Entity"
        ),
        m_entity_remove: need!(method(env, ent, "remove", "()V"), "Entity"),
        m_entity_dead: need!(method(env, ent, "isDead", "()Z"), "Entity"),
        m_entity_persistent: need!(method(env, ent, "setPersistent", "(Z)V"), "Entity"),
        m_entity_invulnerable: opt!(env, ent, "setInvulnerable", "(Z)V"),
        m_world_min_height: opt!(env, w, "getMinHeight", "()I"),
        m_material_name: need!(method(env, mat, "name", "()Ljava/lang/String;"), "Material"),
        m_material_ordinal: need!(method(env, mat, "ordinal", "()I"), "Material"),
        m_material_block_data: need!(
            method(
                env,
                mat,
                "createBlockData",
                "()Lorg/bukkit/block/data/BlockData;"
            ),
            "Material"
        ),
        m_uuid_to_string: need!(method(env, uu, "toString", "()Ljava/lang/String;"), "UUID"),
        m_display_matrix: need!(
            method(
                env,
                disp,
                "setTransformationMatrix",
                "(Lorg/joml/Matrix4f;)V"
            ),
            "Display"
        ),
        m_display_interp_dur: need!(
            method(env, disp, "setInterpolationDuration", "(I)V"),
            "Display"
        ),
        m_display_interp_delay: need!(
            method(env, disp, "setInterpolationDelay", "(I)V"),
            "Display"
        ),
        m_display_teleport_dur: need!(method(env, disp, "setTeleportDuration", "(I)V"), "Display"),
        m_display_brightness: need!(
            method(
                env,
                disp,
                "setBrightness",
                "(Lorg/bukkit/entity/Display$Brightness;)V"
            ),
            "Display"
        ),
        m_block_display_set_block: need!(
            method(env, bd, "setBlock", "(Lorg/bukkit/block/data/BlockData;)V"),
            "BlockDisplay"
        ),
        m_brightness_new: need!(method(env, br, "<init>", "(II)V"), "Brightness"),
        m_matrix_new: need!(method(env, mx, "<init>", "()V"), "Matrix4f"),
        m_matrix_scale: need!(
            method(env, mx, "scale", "(FFF)Lorg/joml/Matrix4f;"),
            "Matrix4f"
        ),

        material_refs: Mutex::new(HashMap::new()),
        ordinal_colors: Mutex::new(HashMap::new()),
        world_refs: Mutex::new(HashMap::new()),

        bukkit,
        location,
        material,
        entity_type,
        brightness,
        uuid,
        matrix,
    };
    Some(Ok(api))
}

// ---------------------------------------------------------------------------
// Read operations (main thread only)
// ---------------------------------------------------------------------------

/// Read all online players' chunk-space positions.
pub fn read_players(api: &BukkitApi, env: &JniEnv) -> Vec<PlayerSnapshot> {
    let mut out = Vec::new();
    let coll = match call_static_obj(
        env,
        api.bukkit.as_jclass(),
        api.m_online_players,
        &[],
        "getOnlinePlayers",
    ) {
        Some(c) => c,
        None => return out,
    };
    let arr = match call_obj(env, coll, api.m_to_array, &[], "Collection.toArray") {
        Some(a) => a,
        None => {
            env.delete_local_ref(coll);
            return out;
        }
    };
    let n = env.get_array_length(arr as jni::jobjectArray);
    for i in 0..n {
        let p = env.get_object_array_element(arr as jni::jobjectArray, i);
        if p.is_null() {
            continue;
        }
        if let Some(snap) = read_player(api, env, p) {
            out.push(snap);
        }
        env.delete_local_ref(p);
    }
    env.delete_local_ref(arr);
    env.delete_local_ref(coll);
    out
}

fn read_player(api: &BukkitApi, env: &JniEnv, player: jni::jobject) -> Option<PlayerSnapshot> {
    let loc = call_obj(env, player, api.m_entity_location, &[], "getLocation")?;
    env.delete_local_ref(loc);
    let world = call_obj(env, loc, api.m_loc_world, &[], "Location.getWorld")?;
    env.delete_local_ref(world);
    let wname = call_obj(env, world, api.m_world_name, &[], "World.getName")?;
    env.delete_local_ref(wname);
    let world_name = jstring_to_string(env, wname as jni::jstring)?;
    let bx = call_int(env, loc, api.m_loc_bx, &[], "getBlockX");
    let bz = call_int(env, loc, api.m_loc_bz, &[], "getBlockZ");
    let uuid_obj = call_obj(env, player, api.m_entity_uuid, &[], "getUniqueId")?;
    let uuid_str_obj = call_obj(env, uuid_obj, api.m_uuid_to_string, &[], "UUID.toString");
    env.delete_local_ref(uuid_obj);
    let uuid = uuid_str_obj.and_then(|o| {
        let s = env.get_string_utf(o as jni::jstring);
        env.delete_local_ref(o);
        s
    })?;
    Some(PlayerSnapshot {
        uuid,
        world: world_name,
        cx: (bx as f64 / 16.0) as f32,
        cz: (bz as f64 / 16.0) as f32,
    })
}

/// Resolve (and cache) a world object by name.
pub fn world_obj(api: &BukkitApi, env: &JniEnv, name: &str) -> Option<jni::jobject> {
    if let Some(&w) = api.world_refs.lock().unwrap().get(name) {
        return Some(w);
    }
    let s = new_jstr(env, name)?;
    let w = call_obj(
        env,
        api.server_obj,
        api.m_world_by_name,
        &[jv_o(s)],
        "getWorld",
    )?;
    env.delete_local_ref(s);
    let g = env.new_global_ref(w);
    env.delete_local_ref(w);
    api.world_refs.lock().unwrap().insert(name.to_string(), g);
    Some(g)
}

/// Read one chunk snapshot into a ChunkData (256 top-block samples).
pub fn read_snapshot(
    api: &BukkitApi,
    env: &JniEnv,
    world: &str,
    cx: i32,
    cz: i32,
) -> Option<crate::state::msg::ChunkData> {
    let wobj = world_obj(api, env, world)?;
    let chunk = call_obj(
        env,
        wobj,
        api.m_world_chunk_at,
        &[jv_i(cx), jv_i(cz)],
        "getChunkAt",
    )?;
    env.delete_local_ref(chunk);
    // chunk may be null if the world unloaded it mid-flight
    let snap = call_obj(
        env,
        chunk,
        api.m_chunk_snap,
        &[jv_z(true), jv_z(false), jv_z(false)],
        "getChunkSnapshot",
    )?;
    env.delete_local_ref(snap);

    let mut samples: Box<[(i32, [u8; 3]); 256]> = Box::new([(0, [0, 0, 0]); 256]);
    let min_world_y = api
        .m_world_min_height
        .map(|m| call_int(env, wobj, m, &[], "getMinHeight"))
        .unwrap_or(-64);
    let mut i = 0usize;
    for x in 0..16i32 {
        for z in 0..16i32 {
            let top = call_int(env, snap, api.m_snap_top_y, &[jv_i(x), jv_i(z)], "topY");
            if top < -512 {
                // void column (Bukkit returns min height - 1)
                samples[i] = (i32::MIN, crate::palette::FALLBACK);
            } else {
                let mat = call_obj(
                    env,
                    snap,
                    api.m_snap_block_type,
                    &[jv_i(x), jv_i(top), jv_i(z)],
                    "getBlockType",
                );
                let color = mat
                    .map(|m| {
                        let c = material_color(api, env, m);
                        env.delete_local_ref(m);
                        c
                    })
                    .unwrap_or(crate::palette::FALLBACK);
                samples[i] = (top, color);
            }
            i += 1;
        }
    }
    Some(crate::state::msg::ChunkData {
        world: world.to_string(),
        cx,
        cz,
        samples,
        min_world_y,
    })
}

/// Map color for a Material object, cached by enum ordinal.
fn material_color(api: &BukkitApi, env: &JniEnv, mat: jni::jobject) -> [u8; 3] {
    let ordinal = call_int(env, mat, api.m_material_ordinal, &[], "ordinal");
    if let Some(c) = api.ordinal_colors.lock().unwrap().get(&ordinal) {
        return *c;
    }
    let name = call_obj(env, mat, api.m_material_name, &[], "Material.name")
        .and_then(|s| {
            let out = env.get_string_utf(s as jni::jstring);
            env.delete_local_ref(s);
            out
        })
        .unwrap_or_default();
    let color = crate::palette::map_color(&name.to_ascii_lowercase());
    api.ordinal_colors.lock().unwrap().insert(ordinal, color);
    color
}

/// Resolve a Material object for a lowercase block name (cached global ref).
pub fn material_obj(api: &BukkitApi, env: &JniEnv, block: &str) -> Option<jni::jobject> {
    let key = block.to_ascii_uppercase();
    if let Some(&m) = api.material_refs.lock().unwrap().get(&key) {
        return Some(m);
    }
    let s = new_jstr(env, &key)?;
    let m = call_static_obj(
        env,
        api.material.as_jclass(),
        api.m_material_value_of,
        &[jv_o(s)],
        "Material.valueOf",
    );
    env.delete_local_ref(s);
    let m = m?;
    let g = env.new_global_ref(m);
    env.delete_local_ref(m);
    api.material_refs.lock().unwrap().insert(key, g);
    Some(g)
}
