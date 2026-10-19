//! Display-entity operations (main thread only): spawn, update, despawn.

use crate::jvm::bukkit::BukkitApi;
use crate::jvm::jniutil::*;
use crate::plan::CellVisual;
use crate::quadtree::CellKey;
use jvmti_bindings::prelude::*;

/// Spawn a BlockDisplay impostor for a cell; returns the entity uuid string.
///
/// The box spans the cell rect from its min corner: position
/// `(cx*16, base_y, cz*16)`, scale `(size_blocks, height, size_blocks)`.
/// Interpolation is configured so later transforms glide.
pub fn spawn_display(
    api: &BukkitApi,
    env: &JniEnv,
    key: &CellKey,
    visual: &CellVisual,
    cfg: &crate::config::Config,
) -> Result<String, String> {
    let wobj = super::bukkit::world_obj(api, env, &key.world)
        .ok_or_else(|| format!("world not found: {}", key.world))?;
    let et = env.get_static_object_field(api.entity_type.as_jclass(), api.f_block_display_type);
    swallow(env, "EntityType.BLOCK_DISPLAY");
    if et.is_null() {
        return Err("BLOCK_DISPLAY type is null".into());
    }

    let x = (key.cx * 16) as f64;
    let z = (key.cz * 16) as f64;
    let y = visual.base_y as f64;
    let loc = env
        .new_object(
            api.location.as_jclass(),
            api.m_loc_new,
            &[jv_o(wobj), jv_d(x), jv_d(y), jv_d(z)],
        )
        .ok_or_else(|| {
            swallow(env, "new Location");
            "new Location failed".to_string()
        })?;

    let entity = call_obj(
        env,
        wobj,
        api.m_world_spawn,
        &[jv_o(loc), jv_o(et)],
        "spawnEntity",
    )
    .ok_or_else(|| "spawnEntity returned null".to_string())?;
    env.delete_local_ref(loc);
    env.delete_local_ref(et);

    // immovable, invulnerable, non-persistent (rebuilds after restart)
    call_void(
        env,
        entity,
        api.m_entity_persistent,
        &[jv_z(false)],
        "setPersistent",
    );
    if let Some(inv) = api.m_entity_invulnerable {
        call_void(env, entity, inv, &[jv_z(true)], "setInvulnerable");
    }

    // display metadata
    call_void(
        env,
        entity,
        api.m_display_interp_dur,
        &[jv_i(cfg.interpolation_ticks as i32)],
        "setInterpolationDuration",
    );
    call_void(
        env,
        entity,
        api.m_display_teleport_dur,
        &[jv_i(cfg.teleport_ticks as i32)],
        "setTeleportDuration",
    );
    call_void(
        env,
        entity,
        api.m_display_interp_delay,
        &[jv_i(0)],
        "setInterpolationDelay",
    );
    apply_brightness(api, env, entity, cfg.brightness);
    if let Err(e) = apply_block(api, env, entity, &visual.material) {
        // material resolution failed — entity exists; keep it with default
        // block (stone is applied by apply_block fallback anyway) and log.
        eprintln!("[c-LoD] spawn block apply: {e}");
    }
    apply_transform(api, env, entity, key, visual)?;

    // read back the uuid
    let uuid_obj = call_obj(env, entity, api.m_entity_uuid, &[], "getUniqueId")
        .ok_or_else(|| "spawned entity has no uuid".to_string())?;
    let uuid = call_obj(env, uuid_obj, api.m_uuid_to_string, &[], "UUID.toString")
        .and_then(|s| {
            let out = env.get_string_utf(s as jni::jstring);
            env.delete_local_ref(s);
            out
        })
        .ok_or_else(|| "uuid toString failed".to_string())?;
    env.delete_local_ref(uuid_obj);
    env.delete_local_ref(entity);
    Ok(uuid)
}

/// Refresh an existing display: block, transform, interpolation restart.
pub fn update_display(
    api: &BukkitApi,
    env: &JniEnv,
    uuid: &str,
    key: &CellKey,
    visual: &CellVisual,
) -> Result<(), String> {
    let entity = entity_by_uuid(api, env, uuid)?.ok_or_else(|| format!("entity gone: {uuid}"))?;
    if call_bool(env, entity, api.m_entity_dead, &[], "isDead") {
        env.delete_local_ref(entity);
        return Err(format!("entity dead: {uuid}"));
    }
    let _ = apply_block(api, env, entity, &visual.material);
    call_void(
        env,
        entity,
        api.m_display_interp_delay,
        &[jv_i(0)],
        "setInterpolationDelay",
    );
    apply_transform(api, env, entity, key, visual)?;
    env.delete_local_ref(entity);
    Ok(())
}

/// Remove a display entity (best effort).
pub fn despawn_display(api: &BukkitApi, env: &JniEnv, uuid: &str) -> Result<(), String> {
    if let Some(entity) = entity_by_uuid(api, env, uuid)? {
        call_void(env, entity, api.m_entity_remove, &[], "remove");
        env.delete_local_ref(entity);
    }
    Ok(())
}

/// Resolve an entity by uuid string (Server.getEntity).
pub fn entity_by_uuid(
    api: &BukkitApi,
    env: &JniEnv,
    uuid: &str,
) -> Result<Option<jni::jobject>, String> {
    let s = new_jstr(env, uuid).ok_or("oom")?;
    let u = call_static_obj(
        env,
        api.uuid.as_jclass(),
        api.m_uuid_from_string,
        &[jv_o(s)],
        "UUID.fromString",
    );
    env.delete_local_ref(s);
    let u = u.ok_or_else(|| "bad uuid".to_string())?;
    let e = call_obj(
        env,
        api.server_obj,
        api.m_get_entity,
        &[jv_o(u)],
        "getEntity",
    );
    env.delete_local_ref(u);
    Ok(e)
}

fn apply_brightness(api: &BukkitApi, env: &JniEnv, entity: jni::jobject, level: u8) {
    let br = env
        .new_object(
            api.brightness.as_jclass(),
            api.m_brightness_new,
            &[jv_i(level as i32), jv_i(level as i32)],
        )
        .unwrap_or(std::ptr::null_mut());
    if !br.is_null() {
        call_void(
            env,
            entity,
            api.m_display_brightness,
            &[jv_o(br)],
            "setBrightness",
        );
        env.delete_local_ref(br);
    } else {
        swallow(env, "new Brightness");
    }
}

/// Set the impostor block; unknown names fall back to stone.
fn apply_block(
    api: &BukkitApi,
    env: &JniEnv,
    entity: jni::jobject,
    material: &str,
) -> Result<(), String> {
    let mobj = super::bukkit::material_obj(api, env, material)
        .or_else(|| super::bukkit::material_obj(api, env, "stone"))
        .ok_or_else(|| "no material".to_string())?;
    let bd = call_obj(env, mobj, api.m_material_block_data, &[], "createBlockData")
        .ok_or_else(|| "createBlockData failed".to_string())?;
    call_void(
        env,
        entity,
        api.m_block_display_set_block,
        &[jv_o(bd)],
        "setBlock",
    );
    env.delete_local_ref(bd);
    Ok(())
}

/// Translation+scale matrix: box from the cell origin corner.
fn apply_transform(
    api: &BukkitApi,
    env: &JniEnv,
    entity: jni::jobject,
    key: &CellKey,
    visual: &CellVisual,
) -> Result<(), String> {
    let m = env
        .new_object(api.matrix.as_jclass(), api.m_matrix_new, &[])
        .ok_or_else(|| {
            swallow(env, "new Matrix4f");
            "new Matrix4f failed".to_string()
        })?;
    let sy = visual.height.max(1) as f32;
    let s = key.size_blocks() as f32;
    // display geometry is a unit block at the entity origin; scale + tiny
    // translation to counter sub-block seams between neighbouring cells
    let eps = 0.02f32;
    call_obj(
        env,
        m,
        api.m_matrix_scale,
        &[jv_f(s + eps), jv_f(sy), jv_f(s + eps)],
        "Matrix4f.scale",
    )
    .ok_or_else(|| "scale failed".to_string())?;
    call_void(
        env,
        entity,
        api.m_display_matrix,
        &[jv_o(m)],
        "setTransformationMatrix",
    );
    env.delete_local_ref(m);
    Ok(())
}
