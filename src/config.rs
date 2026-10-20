//! c-LoD configuration: parsed from the CRUSSTY module options string.
//!
//! Options format: `key1=value1;key2=value2;...` (whatever the runtime hands
//! to `cplugin_init`). Every knob has a working default, so an empty options
//! string produces a functional module.

use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct Config {
    /// Port of the embedded HTTP server that serves the generated resource
    /// pack. The pack URL becomes `pack_base_url + "/clod-pack.zip"`.
    pub http_port: u16,
    /// Base URL clients can reach this server on, e.g. `http://play.example.com`.
    /// If empty, the module auto-detects the first non-loopback IPv4 address.
    pub pack_base_url: String,
    /// Resource pack UUID pushed to clients. Stable across reboots so vanilla
    /// clients only re-download when the SHA-1 changes.
    pub pack_uuid: String,
    /// Prompt shown by the client for the resource pack.
    pub pack_prompt: String,
    /// World used for the heightmap atlas (empty = first Bukkit world).
    pub world: String,
    /// Atlas center in block coordinates (default: world spawn).
    pub center_x: i32,
    pub center_z: i32,
    /// Atlas side length in blocks (atlas covers [-size/2, +size/2)).
    /// Must be a power of two, 512..=16384.
    pub atlas_size: i32,
    /// One atlas texel covers `cell` x `cell` blocks (1, 2, 4, 8...).
    pub cell_size: i32,
    /// Heightmap cells scanned per engine tick (main-thread JNI budget).
    pub scan_budget: u32,
    /// Static-pack scan deadline: seconds from atlas-world resolution until
    /// the heightmap sweep is finalized with whatever it has. The pack is
    /// built EXACTLY ONCE per session — either when the sweep completes or
    /// when this deadline fires, never before, never again.
    pub scan_deadline_secs: u64,
    /// Persist the finalized atlas to `atlas_path` (default
    /// `c-LoD-atlas.bin` next to the server) so restarts rebuild the exact
    /// same pack bytes (same SHA-1) and vanilla clients never re-download.
    pub atlas_cache: bool,
    /// Where the atlas snapshot is persisted.
    pub atlas_path: String,
    /// Warm-up radius in CHUNKS around the atlas center: at world resolution
    /// the engine synchronously loads/generates (2*warm+1)^2 chunks so the
    /// static snapshot always covers the core region (modern vanilla keeps
    /// no spawn chunks loaded, and the scan never forces generation).
    pub warm_chunks: i32,
    /// Camera FOV (vertical, degrees) the GLSL ray reconstruction assumes.
    /// Must match the client's FOV for pixel-perfect alignment; 70 is vanilla.
    pub fov: f32,
    /// Distance (blocks) where the LOD raymarch starts. Keep it BELOW the
    /// client view distance: the chain only paints sky-depth pixels, so real
    /// chunks always win by depth and the LOD fills underneath the chunk edge
    /// — the seamless blend DH-style packs achieve (a start above the view
    /// distance leaves the dreaded gap between real terrain and LOD).
    pub lod_near: f32,
    /// Effective maximum distance of the atmosphere veil (blocks).
    pub veil_far: f32,
    /// Sea level used for water flagging and LOD water rendering.
    pub sea_level: i32,
    /// Engine tick interval in milliseconds.
    pub tick_ms: u64,
    /// Post-effect ids (namespace) installed into the generated pack.
    pub effect_terrain: String,
    pub effect_atmosphere: String,
    /// Palette: LOD terrain colors (linear RGB 0..1).
    pub color_low: [f32; 3],
    pub color_high: [f32; 3],
    pub color_water: [f32; 3],
    pub snow_line: f32,
    /// Sky/veil color blended over the render-distance edge.
    pub color_veil: [f32; 3],
    /// Sun tracking: warmth threshold and temporal blend factor.
    pub sun_threshold: f32,
    pub rot_blend: f32,
    /// Upper bound on terrain height used to cull rays early.
    pub max_height: i32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            http_port: 8199,
            pack_base_url: String::new(),
            pack_uuid: "6b1f2f7e-9c0d-4a4b-8b0a-b10d1b4ab100".to_string(),
            pack_prompt: String::new(),
            world: String::new(),
            center_x: 0,
            center_z: 0,
            atlas_size: 4096,
            cell_size: 4,
            scan_budget: 4096,
            scan_deadline_secs: 900,
            atlas_cache: true,
            atlas_path: "c-LoD-atlas.bin".to_string(),
            warm_chunks: 8,
            fov: 70.0,
            lod_near: 96.0,
            veil_far: 512.0,
            sea_level: 63,
            tick_ms: 1000,
            effect_terrain: "clod:lod_terrain".to_string(),
            effect_atmosphere: "clod:atmosphere".to_string(),
            color_low: [0.35, 0.52, 0.28],
            color_high: [0.48, 0.44, 0.40],
            color_water: [0.12, 0.24, 0.45],
            snow_line: 0.78,
            color_veil: [0.62, 0.74, 0.86],
            sun_threshold: 0.18,
            rot_blend: 0.35,
            max_height: 320,
        }
    }
}

impl Config {
    pub fn parse(options: &str) -> Config {
        let mut cfg = Config::default();
        let mut kv: HashMap<&str, &str> = HashMap::new();
        for part in options.split([';', ',', '\n']) {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            if let Some((k, v)) = part.split_once('=') {
                kv.insert(k.trim(), v.trim());
            }
        }
        let get = |k: &str| kv.get(k).copied();
        if let Some(v) = get("http_port").and_then(|v| v.parse().ok()) {
            cfg.http_port = v;
        }
        if let Some(v) = get("pack_base_url") {
            cfg.pack_base_url = v.trim_end_matches('/').to_string();
        }
        if let Some(v) = get("pack_uuid") {
            cfg.pack_uuid = v.to_string();
        }
        if let Some(v) = get("pack_prompt") {
            cfg.pack_prompt = v.to_string();
        }
        if let Some(v) = get("world") {
            cfg.world = v.to_string();
        }
        if let Some(v) = get("center_x").and_then(|v| v.parse::<i32>().ok()) {
            cfg.center_x = v;
        }
        if let Some(v) = get("center_z").and_then(|v| v.parse::<i32>().ok()) {
            cfg.center_z = v;
        }
        if let Some(v) = get("atlas_size").and_then(|v| v.parse::<i32>().ok()) {
            if (512..=16384).contains(&v) && v > 0 && (v & (v - 1)) == 0 {
                cfg.atlas_size = v;
            }
        }
        if let Some(v) = get("cell_size").and_then(|v| v.parse::<i32>().ok()) {
            if v > 0 && (v & (v - 1)) == 0 && (1..=32).contains(&v) {
                cfg.cell_size = v;
            }
        }
        if let Some(v) = get("scan_budget").and_then(|v| v.parse::<u32>().ok()) {
            cfg.scan_budget = v.clamp(64, 65536);
        }
        if let Some(v) = get("scan_deadline_secs").and_then(|v| v.parse::<u64>().ok()) {
            cfg.scan_deadline_secs = v.clamp(10, 86400);
        }
        if let Some(v) = get("atlas_cache") {
            cfg.atlas_cache = !matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "false" | "0" | "off" | "no"
            );
        }
        if let Some(v) = get("atlas_path") {
            let v = v.trim();
            if !v.is_empty() {
                cfg.atlas_path = v.to_string();
            }
        }
        if let Some(v) = get("warm_chunks").and_then(|v| v.parse::<i32>().ok()) {
            cfg.warm_chunks = v.clamp(0, 32);
        }
        if let Some(v) = get("fov").and_then(|v| v.parse::<f32>().ok()) {
            if (30.0..=150.0).contains(&v) {
                cfg.fov = v;
            }
        }
        if let Some(v) = get("lod_near").and_then(|v| v.parse::<f32>().ok()) {
            if (64.0..=8192.0).contains(&v) {
                cfg.lod_near = v;
            }
        }
        if let Some(v) = get("veil_far").and_then(|v| v.parse::<f32>().ok()) {
            if (128.0..=16384.0).contains(&v) {
                cfg.veil_far = v;
            }
        }
        if let Some(v) = get("sea_level").and_then(|v| v.parse::<i32>().ok()) {
            cfg.sea_level = v;
        }
        if let Some(v) = get("tick_ms").and_then(|v| v.parse::<u64>().ok()) {
            cfg.tick_ms = v.clamp(200, 60_000);
        }
        if let Some(v) = get("effect_terrain") {
            cfg.effect_terrain = v.to_string();
        }
        if let Some(v) = get("effect_atmosphere") {
            cfg.effect_atmosphere = v.to_string();
        }
        if let Some(v) = parse_color(get("color_low")) {
            cfg.color_low = v;
        }
        if let Some(v) = parse_color(get("color_high")) {
            cfg.color_high = v;
        }
        if let Some(v) = parse_color(get("color_water")) {
            cfg.color_water = v;
        }
        if let Some(v) = parse_color(get("color_veil")) {
            cfg.color_veil = v;
        }
        if let Some(v) = get("snow_line").and_then(|v| v.parse::<f32>().ok()) {
            cfg.snow_line = v.clamp(0.0, 1.0);
        }
        if let Some(v) = get("sun_threshold").and_then(|v| v.parse::<f32>().ok()) {
            cfg.sun_threshold = v.clamp(0.0, 1.0);
        }
        if let Some(v) = get("rot_blend").and_then(|v| v.parse::<f32>().ok()) {
            cfg.rot_blend = v.clamp(0.02, 1.0);
        }
        if let Some(v) = get("max_height").and_then(|v| v.parse::<i32>().ok()) {
            cfg.max_height = v.clamp(64, 1024);
        }
        cfg
    }
}

fn parse_color(v: Option<&str>) -> Option<[f32; 3]> {
    let v = v?;
    let mut out = [0f32; 3];
    for (i, c) in v.split(['/', ':', ',']).take(3).enumerate() {
        out[i] = c.trim().parse::<f32>().ok()?;
    }
    // Accept 0..255 ranges too: if any component > 1, scale all.
    if out.iter().any(|c| *c > 1.0) {
        for c in &mut out {
            *c /= 255.0;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sane() {
        let c = Config::parse("");
        assert_eq!(c.http_port, 8199);
        assert_eq!(c.atlas_size, 4096);
        assert_eq!(c.cell_size, 4);
        assert_eq!(c.fov, 70.0);
    }

    #[test]
    fn parses_everything() {
        let c = Config::parse(
            "http_port=9001;fov=90;atlas_size=8192;cell_size=2;color_low=0.1/0.8/0.2;world=world_nether",
        );
        assert_eq!(c.http_port, 9001);
        assert_eq!(c.fov, 90.0);
        assert_eq!(c.atlas_size, 8192);
        assert_eq!(c.cell_size, 2);
        assert_eq!(c.color_low, [0.1, 0.8, 0.2]);
        assert_eq!(c.world, "world_nether");
    }

    #[test]
    fn rejects_bad_atlas_size() {
        let c = Config::parse("atlas_size=3000");
        assert_eq!(c.atlas_size, 4096);
    }

    #[test]
    fn static_pack_options() {
        let c = Config::parse("");
        assert_eq!(c.scan_deadline_secs, 900);
        assert!(c.atlas_cache);
        assert_eq!(c.atlas_path, "c-LoD-atlas.bin");
        assert_eq!(c.warm_chunks, 8);

        let c = Config::parse("scan_deadline_secs=7;atlas_cache=off;atlas_path=/tmp/atlas.bin");
        assert_eq!(c.scan_deadline_secs, 10, "clamped to minimum");
        assert!(!c.atlas_cache);
        assert_eq!(c.atlas_path, "/tmp/atlas.bin");
    }

    #[test]
    fn color_scale_255() {
        let c = Config::parse("color_veil=160/190/220");
        assert!((c.color_veil[0] - 160.0 / 255.0).abs() < 1e-4);
    }
}
