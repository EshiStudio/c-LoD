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
    /// Seconds between full atlas refresh sweeps.
    pub refresh_secs: u64,
    /// Camera FOV (vertical, degrees) the GLSL ray reconstruction assumes.
    /// Must match the client's FOV for pixel-perfect alignment; 70 is vanilla.
    pub fov: f32,
    /// Distance (blocks) where the LOD raymarch starts. Keep above the
    /// server's view distance so real chunks never overlap LOD.
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
    /// Number of atmosphere veil tiers emitted into the pack (1..=8).
    /// Tier 0 is the weakest veil (far visibility), tier N-1 the strongest
    /// (ground level). Requires `adaptive` to matter.
    pub atmo_tiers: u32,
    /// Altitude-adaptive LOD: pick the veil tier per player from the height
    /// above terrain (high above ground -> weaker veil -> see further).
    /// When false, covered players always get the strongest tier.
    pub adaptive: bool,
    /// Recenter the atlas window when all players drift this many blocks
    /// outside it (Chebyshev). 0 disables recentering.
    pub recenter_blocks: i32,
    /// Consecutive adaptive ticks all players must stay outside the window
    /// before the atlas recenters (anti-jitter patience).
    pub recenter_patience: u32,
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
            refresh_secs: 300,
            fov: 70.0,
            lod_near: 256.0,
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
            atmo_tiers: 3,
            adaptive: true,
            recenter_blocks: 1024,
            recenter_patience: 20,
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
        if let Some(v) = get("refresh_secs").and_then(|v| v.parse::<u64>().ok()) {
            cfg.refresh_secs = v.clamp(30, 86400);
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
        if let Some(v) = get("atmo_tiers").and_then(|v| v.parse::<u32>().ok()) {
            cfg.atmo_tiers = v.clamp(1, 8);
        }
        if let Some(v) = get("adaptive") {
            cfg.adaptive = !matches!(
                v.to_ascii_lowercase().as_str(),
                "false" | "0" | "off" | "no"
            );
        }
        if let Some(v) = get("recenter_blocks").and_then(|v| v.parse::<i32>().ok()) {
            cfg.recenter_blocks = v.clamp(0, 32_768);
        }
        if let Some(v) = get("recenter_patience").and_then(|v| v.parse::<u32>().ok()) {
            cfg.recenter_patience = v.clamp(1, 3600);
        }
        cfg
    }
}

/// Map "height above terrain" (blocks) to a veil tier index in
/// `0..=max` (0 = weakest veil, max = strongest). High above ground ->
/// tier 0; near/below ground -> max. Pure so tests cover it without state.
pub fn tier_from_delta(delta: i32, max: u32) -> u32 {
    if max == 0 {
        return 0;
    }
    let d = delta.clamp(0, 1_000_000);
    // Band width grows from 8 blocks up: 0..31 -> strongest, 32..55 -> next,
    // and so on until the weakest tier.
    let level = ((d - 8).max(0) / 24) as u32;
    max - level.min(max)
}

#[cfg(test)]
mod tier_tests {
    use super::tier_from_delta;

    #[test]
    fn tier_bands() {
        // 3 tiers: max index 2.
        assert_eq!(tier_from_delta(0, 2), 2); // ground level
        assert_eq!(tier_from_delta(8, 2), 2);
        assert_eq!(tier_from_delta(31, 2), 2);
        assert_eq!(tier_from_delta(32, 2), 1);
        assert_eq!(tier_from_delta(55, 2), 1);
        assert_eq!(tier_from_delta(56, 2), 0);
        assert_eq!(tier_from_delta(400, 2), 0); // high above ground
    }

    #[test]
    fn tier_single() {
        assert_eq!(tier_from_delta(-5, 0), 0);
        assert_eq!(tier_from_delta(100, 0), 0);
    }

    #[test]
    fn tier_negative_clamps() {
        assert_eq!(tier_from_delta(-100, 3), 3);
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
    fn color_scale_255() {
        let c = Config::parse("color_veil=160/190/220");
        assert!((c.color_veil[0] - 160.0 / 255.0).abs() < 1e-4);
    }
}
