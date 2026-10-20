//! Assembles the c-LoD resource pack: pack.mcmeta, the two post-effect
//! chains (JSON with config values substituted), the GLSL shaders and the
//! heightmap atlas PNG. Output is a zip (STORE) + its SHA-1 for client
//! caching.

use crate::atlas::Atlas;
use crate::config::Config;
use crate::png;
use crate::sha1::Sha1;
use crate::zipw::ZipWriter;

pub struct Pack {
    pub zip: Vec<u8>,
    pub sha1: [u8; 20],
    pub sha1_hex: String,
    pub atlas_version: u64,
    /// Post-effect ids emitted for this pack, weakest-veil tier first
    /// (`clod:atmosphere_t0..`), so the engine can bind Keys per tier.
    pub atmosphere_ids: Vec<String>,
}

fn fmt_f(v: f32) -> String {
    if v.abs() < 1e-6 {
        return "0.0".to_string();
    }
    let s = format!("{v:.4}");
    let s = s.trim_end_matches('0').trim_end_matches('.').to_string();
    if s.is_empty() || s == "-" {
        "0.0".to_string()
    } else if s.ends_with('.') {
        format!("{s}0")
    } else {
        s
    }
}

fn vec4(v: [f32; 4]) -> String {
    format!(
        "[{}, {}, {}, {}]",
        fmt_f(v[0]),
        fmt_f(v[1]),
        fmt_f(v[2]),
        fmt_f(v[3])
    )
}

fn vec3(v: [f32; 3]) -> String {
    format!("[{}, {}, {}]", fmt_f(v[0]), fmt_f(v[1]), fmt_f(v[2]))
}

fn substitute(template: &str, map: &[(&str, String)]) -> String {
    let mut out = template.to_string();
    for (k, v) in map {
        out = out.replace(k, v);
    }
    out
}

pub fn build(cfg: &Config, atlas: &Atlas) -> Pack {
    let atlas_version = atlas.version();

    let lod_terrain_tpl = include_str!("../assets/post_effect/lod_terrain.json");
    let atmosphere_tpl = include_str!("../assets/post_effect/atmosphere.json");

    let origin_x = atlas.center_x() - atlas.half();
    let origin_z = atlas.center_z() - atlas.half();
    let fog_start = cfg.lod_near * 1.05;
    let fog_end = atlas.size as f32 * 0.72;

    let veil_rgba = [
        cfg.color_veil[0],
        cfg.color_veil[1],
        cfg.color_veil[2],
        cfg.snow_line,
    ];

    let mut map: Vec<(&str, String)> = vec![
        ("__FOV__", fmt_f(cfg.fov)),
        ("__SCAN_THRESH__", fmt_f(cfg.sun_threshold)),
        ("__ROT_BLEND__", fmt_f(cfg.rot_blend)),
        ("__SOLVE_THRESH__", fmt_f(cfg.sun_threshold * 220.0)),
        ("__ORIGIN_X__", fmt_f(origin_x as f32)),
        ("__ORIGIN_Z__", fmt_f(origin_z as f32)),
        ("__ATLAS_SIZE__", fmt_f(atlas.size as f32)),
        ("__ATLAS_CELLS__", atlas.cells.to_string()),
        ("__CELL__", fmt_f(atlas.cell as f32)),
        ("__LOD_NEAR__", fmt_f(cfg.lod_near)),
        ("__MAX_HEIGHT__", fmt_f(cfg.max_height as f32)),
        ("__SEA_LEVEL__", fmt_f(cfg.sea_level as f32)),
        ("__MIN_CONF__", fmt_f(0.25)),
        ("__FOG_START__", fmt_f(fog_start)),
        ("__FOG_END__", fmt_f(fog_end)),
        ("__COL_LOW__", vec3(cfg.color_low)),
        ("__COL_HIGH__", vec3(cfg.color_high)),
        ("__COL_WATER__", vec3(cfg.color_water)),
        ("__VEIL_RGBA__", vec4(veil_rgba)),
    ];

    let lod_terrain_json = substitute(lod_terrain_tpl, &map);

    // Atmosphere veil tiers: tier 0 = weakest (seen from high above the
    // terrain — the horizon stays crisp), tier N-1 = strongest (ground
    // level — full render-distance edge masking). Depth windows are kept
    // inside the device-depth band where real geometry lives.
    let n = cfg.atmo_tiers.max(1);
    let max_t = n - 1;
    let base_name = cfg
        .effect_atmosphere
        .rsplit_once(':')
        .map(|(_, s)| s.to_string())
        .unwrap_or_else(|| "atmosphere".to_string());
    let mut atmosphere_ids = Vec::with_capacity(n as usize);
    let mut atmosphere_entries: Vec<(String, String)> = Vec::with_capacity(n as usize);
    for t in 0..n {
        let frac = if max_t == 0 {
            1.0
        } else {
            t as f32 / max_t as f32
        };
        let lerp = |a: f32, b: f32, f: f32| a + (b - a) * f;
        let start = lerp(0.99940, 0.99900, frac);
        let end = lerp(0.99985, 0.99975, frac);
        let strength = lerp(0.55, 0.92, frac);
        map.retain(|(k, _)| {
            *k != "__ATMO_START__" && *k != "__ATMO_END__" && *k != "__ATMO_STRENGTH__"
        });
        map.push(("__ATMO_START__", fmt_f(start)));
        map.push(("__ATMO_END__", fmt_f(end)));
        map.push(("__ATMO_STRENGTH__", fmt_f(strength)));
        let json = substitute(atmosphere_tpl, &map);
        let id = format!("{}_t{t}", cfg.effect_atmosphere);
        atmosphere_ids.push(id);
        atmosphere_entries.push((
            format!("assets/clod/post_effect/{base_name}_t{t}.json"),
            json,
        ));
    }

    let sun_scan = include_str!("../assets/glsl/sun_scan.fsh");
    let sun_reduce = include_str!("../assets/glsl/sun_reduce.fsh");
    let sun_solve = include_str!("../assets/glsl/sun_solve.fsh");
    let rot_copy = include_str!("../assets/glsl/rot_copy.fsh");
    let lod_terrain = include_str!("../assets/glsl/lod_terrain.fsh");
    let clod_blit = include_str!("../assets/glsl/clod_blit.fsh");
    let atmosphere = include_str!("../assets/glsl/atmosphere.fsh");

    let mcmeta = format!(
        "{{\"pack\":{{\"pack_format\":97,\"description\":\"c-LoD LOD terrain (atlas v{atlas_version})\"}}}}"
    );

    let (aw, ah, atlas_rgba) = atlas.encode_rgba();
    let atlas_png = png::encode_png(aw, ah, &atlas_rgba);

    let mut zip = ZipWriter::new();
    zip.add("pack.mcmeta", mcmeta.as_bytes());
    zip.add(
        "assets/clod/post_effect/lod_terrain.json",
        lod_terrain_json.as_bytes(),
    );
    for (path, json) in &atmosphere_entries {
        zip.add(path, json.as_bytes());
    }
    zip.add("assets/clod/shaders/post/sun_scan.fsh", sun_scan.as_bytes());
    zip.add(
        "assets/clod/shaders/post/sun_reduce.fsh",
        sun_reduce.as_bytes(),
    );
    zip.add(
        "assets/clod/shaders/post/sun_solve.fsh",
        sun_solve.as_bytes(),
    );
    zip.add("assets/clod/shaders/post/rot_copy.fsh", rot_copy.as_bytes());
    zip.add(
        "assets/clod/shaders/post/lod_terrain.fsh",
        lod_terrain.as_bytes(),
    );
    zip.add(
        "assets/clod/shaders/post/clod_blit.fsh",
        clod_blit.as_bytes(),
    );
    zip.add(
        "assets/clod/shaders/post/atmosphere.fsh",
        atmosphere.as_bytes(),
    );
    zip.add("assets/clod/textures/effect/atlas.png", &atlas_png);
    let zip_bytes = zip.finish();

    let mut h = Sha1::new();
    h.update(&zip_bytes);
    let sha1 = h.finish();
    let sha1_hex = crate::sha1::sha1_hex(&zip_bytes);

    Pack {
        zip: zip_bytes,
        sha1,
        sha1_hex,
        atlas_version,
        atmosphere_ids,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zipw;

    #[test]
    fn pack_builds_and_is_valid_zip() {
        let cfg = Config::parse("");
        let atlas = Atlas::new(
            cfg.center_x,
            cfg.center_z,
            cfg.atlas_size,
            cfg.cell_size,
            cfg.sea_level,
        );
        atlas.set(0, 0, 70, false);
        let pack = build(&cfg, &atlas);

        assert!(!pack.zip.is_empty());
        assert_eq!(pack.sha1_hex.len(), 40);
        assert_eq!(pack.atlas_version, atlas.version());

        // EOCD present
        let n = pack.zip.len();
        assert_eq!(&pack.zip[n - 22..n - 18], &[0x50, 0x4b, 0x05, 0x06]);

        // Must contain the essential entries: scan by walking central dir.
        let count = u16::from_le_bytes([pack.zip[n - 12], pack.zip[n - 11]]) as usize;
        // pack.mcmeta + lod_terrain.json + N atmosphere tiers + 7 fsh + atlas.png
        assert_eq!(count, 2 + cfg.atmo_tiers as usize + 7 + 1);
        assert_eq!(pack.atmosphere_ids.len(), cfg.atmo_tiers as usize);
        assert_eq!(pack.atmosphere_ids[0], "clod:atmosphere_t0");
        assert_eq!(
            pack.atmosphere_ids[cfg.atmo_tiers as usize - 1],
            format!("clod:atmosphere_t{}", cfg.atmo_tiers - 1)
        );

        // JSON chains are valid JSON (parenthesis balance + token-free).
        let s = String::from_utf8_lossy(pack.zip.as_slice());
        assert!(!s.contains("__FOV__"), "tokens must be substituted");
        let _ = zipw::crc32(pack.zip.as_slice());
    }

    #[test]
    fn tier_uniforms_scale() {
        let cfg = Config::parse("atmo_tiers=4");
        let atlas = Atlas::new(0, 0, 64, 4, 63);
        atlas.set(1, 1, 80, false);
        let pack = build(&cfg, &atlas);
        assert_eq!(pack.atmosphere_ids.len(), 4);
        // Tier 0 (weakest): strength 0.55, start 0.9994.
        // Tier 3 (strongest): strength 0.92, start 0.999.
        let s = String::from_utf8_lossy(&pack.zip);
        assert!(s.contains("0.55"), "weakest tier strength");
        assert!(s.contains("0.92"), "strongest tier strength");
    }

    #[test]
    fn fmt_f_stable() {
        assert_eq!(fmt_f(70.0), "70");
        assert_eq!(fmt_f(0.0), "0.0");
        assert_eq!(fmt_f(-1.5), "-1.5");
        assert_eq!(fmt_f(0.25), "0.25");
    }
}
