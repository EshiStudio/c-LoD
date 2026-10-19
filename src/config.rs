//! c-LoD configuration: `clod.conf` next to the module (key = value lines,
//! `#` comments), strict parsing with a full set of defaults so the module
//! runs with zero configuration.

use std::collections::HashMap;

/// Ticket pinning backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TicketsBackend {
    /// Probe NMS TicketStorage; fall back to forceload if the chain breaks.
    Auto,
    /// NMS TicketStorage (PLAYER_LOADING, FULL_CHUNK_LEVEL) — load, no ticking.
    Nms,
    /// Bukkit World#setChunkForceLoaded — persistent, entity-ticking.
    ForceLoad,
    /// Never pin chunks: displays only in already-loaded chunks.
    None,
}

/// Runtime configuration (all values are final after boot).
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// Far LOD distance in blocks (cells are planned up to this radius).
    pub max_distance: u32,
    /// Server view distance (chunks) — vanilla range; LODs start right past it.
    /// 0 = read live from the server at boot.
    pub view_distance: u32,
    /// Annulus growth factor: each LOD level spans `growth *` the distance of
    /// the previous one (2.0 keeps screen-space density roughly constant).
    pub growth: f32,
    /// Hysteresis band (fraction of the level boundary) to stop cells from
    /// flickering when the player walks along a boundary.
    pub hysteresis: f32,
    /// Engine poll interval (ms). Player positions are read once per interval.
    pub update_interval_ms: u64,
    /// Per-cycle JVM budgets (ops executed on the main thread per cycle).
    pub snapshots_per_cycle: u32,
    pub spawns_per_cycle: u32,
    pub updates_per_cycle: u32,
    pub despawns_per_cycle: u32,
    pub pins_per_cycle: u32,
    pub unpins_per_cycle: u32,
    /// Hard cap on active LOD cells (safety net over per-player budgets).
    pub max_cells_total: u32,
    /// Display interpolation (ticks). 0 disables interpolation.
    pub interpolation_ticks: u32,
    /// Display teleport interpolation (ticks, vanilla caps at 59).
    pub teleport_ticks: u32,
    /// Block/sky light override for displays (vanilla max 15).
    pub brightness: u8,
    /// Re-snapshot active LOD0 cells after this many seconds (world edits).
    pub refresh_secs: u64,
    /// Chunk pinning backend.
    pub tickets: TicketsBackend,
    /// Pre-boot spigot.yml fixup: raise entity-tracking-range.display to the
    /// LOD distance so far displays are actually tracked to clients.
    pub tracking_raise: bool,
    /// Log line every N engine cycles.
    pub log_interval_cycles: u32,
    /// Maximum chunks whose snapshot may be read per player cycle (lag guard).
    pub max_cells_per_player: u32,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            max_distance: 1024,
            view_distance: 0,
            growth: 2.0,
            hysteresis: 0.05,
            update_interval_ms: 500,
            snapshots_per_cycle: 128,
            spawns_per_cycle: 64,
            updates_per_cycle: 64,
            despawns_per_cycle: 128,
            pins_per_cycle: 32,
            unpins_per_cycle: 64,
            max_cells_total: 4096,
            interpolation_ticks: 8,
            teleport_ticks: 8,
            brightness: 15,
            refresh_secs: 60,
            tickets: TicketsBackend::Auto,
            tracking_raise: true,
            log_interval_cycles: 40,
            max_cells_per_player: 512,
        }
    }
}

impl Config {
    /// Parse a `clod.conf` payload. Unknown keys are errors (typo protection),
    /// invalid values are errors, everything else falls back to defaults.
    pub fn parse(text: &str) -> Result<Config, String> {
        let mut cfg = Config::default();
        let mut seen: HashMap<&str, ()> = HashMap::new();
        for (lineno, raw) in text.lines().enumerate() {
            let line = strip_comment(raw).trim();
            if line.is_empty() {
                continue;
            }
            let (key, value) = split_kv(line, lineno)?;
            if seen.insert(key, ()).is_some() {
                return Err(format!("line {}: duplicate key `{}`", lineno + 1, key));
            }
            apply(&mut cfg, key, value, lineno)?;
        }
        cfg.validate()?;
        Ok(cfg)
    }
}

impl Config {
    fn validate(&self) -> Result<(), String> {
        if self.max_distance < 128 {
            return Err("max_distance must be >= 128 blocks".into());
        }
        if self.max_distance > 32768 {
            return Err("max_distance must be <= 32768 blocks".into());
        }
        if !(1.3..=4.0).contains(&self.growth) {
            return Err("growth must be within [1.3, 4.0]".into());
        }
        if !(0.0..=0.3).contains(&self.hysteresis) {
            return Err("hysteresis must be within [0.0, 0.3]".into());
        }
        if self.update_interval_ms < 50 {
            return Err("update_interval_ms must be >= 50".into());
        }
        if self.interpolation_ticks > 59 {
            return Err("interpolation_ticks must be <= 59 (vanilla display cap)".into());
        }
        if self.teleport_ticks > 59 {
            return Err("teleport_ticks must be <= 59 (vanilla display cap)".into());
        }
        if self.brightness > 15 {
            return Err("brightness must be <= 15".into());
        }
        Ok(())
    }
}

fn strip_comment(line: &str) -> &str {
    match line.find('#') {
        Some(i) => &line[..i],
        None => line,
    }
}

fn split_kv(line: &str, lineno: usize) -> Result<(&str, &str), String> {
    let (k, v) = line
        .split_once('=')
        .ok_or_else(|| format!("line {}: expected `key = value`", lineno + 1))?;
    let k = k.trim();
    let v = v.trim();
    if k.is_empty() || v.is_empty() {
        return Err(format!("line {}: empty key or value", lineno + 1));
    }
    Ok((k, v))
}

fn parse_num<T: std::str::FromStr>(v: &str, key: &str) -> Result<T, String> {
    v.parse::<T>()
        .map_err(|_| format!("key `{}`: `{}` is not a valid number", key, v))
}

fn parse_backend(v: &str) -> Result<TicketsBackend, String> {
    match v {
        "auto" => Ok(TicketsBackend::Auto),
        "nms" => Ok(TicketsBackend::Nms),
        "forceload" => Ok(TicketsBackend::ForceLoad),
        "none" => Ok(TicketsBackend::None),
        other => Err(format!(
            "tickets: `{}` is not one of auto|nms|forceload|none",
            other
        )),
    }
}

fn apply(cfg: &mut Config, key: &str, v: &str, lineno: usize) -> Result<(), String> {
    let ctx = |e: String| format!("line {}: {}", lineno + 1, e);
    match key {
        "max_distance" => cfg.max_distance = parse_num(v, key).map_err(ctx)?,
        "view_distance" => cfg.view_distance = parse_num(v, key).map_err(ctx)?,
        "growth" => cfg.growth = parse_num(v, key).map_err(ctx)?,
        "hysteresis" => cfg.hysteresis = parse_num(v, key).map_err(ctx)?,
        "update_interval_ms" => cfg.update_interval_ms = parse_num(v, key).map_err(ctx)?,
        "snapshots_per_cycle" => cfg.snapshots_per_cycle = parse_num(v, key).map_err(ctx)?,
        "spawns_per_cycle" => cfg.spawns_per_cycle = parse_num(v, key).map_err(ctx)?,
        "updates_per_cycle" => cfg.updates_per_cycle = parse_num(v, key).map_err(ctx)?,
        "despawns_per_cycle" => cfg.despawns_per_cycle = parse_num(v, key).map_err(ctx)?,
        "pins_per_cycle" => cfg.pins_per_cycle = parse_num(v, key).map_err(ctx)?,
        "unpins_per_cycle" => cfg.unpins_per_cycle = parse_num(v, key).map_err(ctx)?,
        "max_cells_total" => cfg.max_cells_total = parse_num(v, key).map_err(ctx)?,
        "interpolation_ticks" => cfg.interpolation_ticks = parse_num(v, key).map_err(ctx)?,
        "teleport_ticks" => cfg.teleport_ticks = parse_num(v, key).map_err(ctx)?,
        "brightness" => cfg.brightness = parse_num(v, key).map_err(ctx)?,
        "refresh_secs" => cfg.refresh_secs = parse_num(v, key).map_err(ctx)?,
        "tickets" => cfg.tickets = parse_backend(v).map_err(ctx)?,
        "tracking_raise" => cfg.tracking_raise = parse_bool(v).map_err(ctx)?,
        "log_interval_cycles" => cfg.log_interval_cycles = parse_num(v, key).map_err(ctx)?,
        "max_cells_per_player" => cfg.max_cells_per_player = parse_num(v, key).map_err(ctx)?,
        other => return Err(format!("line {}: unknown key `{}`", lineno + 1, other)),
    }
    Ok(())
}

fn parse_bool(v: &str) -> Result<bool, String> {
    match v {
        "true" | "yes" | "1" => Ok(true),
        "false" | "no" | "0" => Ok(false),
        other => Err(format!("`{}` is not a boolean (true|false)", other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_parse_empty() {
        let cfg = Config::parse("").unwrap();
        assert_eq!(cfg, Config::default());
        // comments + blank lines only
        let cfg = Config::parse("# hi\n\n   \n# more\n").unwrap();
        assert_eq!(cfg, Config::default());
    }

    #[test]
    fn full_override_roundtrip() {
        let text = "\
max_distance = 2048
view_distance = 12
growth = 1.5
hysteresis = 0.1
update_interval_ms = 250
snapshots_per_cycle = 64
spawns_per_cycle = 32
updates_per_cycle = 33
despawns_per_cycle = 34
pins_per_cycle = 16
unpins_per_cycle = 17
max_cells_total = 1000
interpolation_ticks = 2
teleport_ticks = 3
brightness = 12
refresh_secs = 30
tickets = forceload
tracking_raise = false
log_interval_cycles = 5
max_cells_per_player = 100
";
        let cfg = Config::parse(text).unwrap();
        assert_eq!(cfg.max_distance, 2048);
        assert_eq!(cfg.view_distance, 12);
        assert!((cfg.growth - 1.5).abs() < 1e-6);
        assert!((cfg.hysteresis - 0.1).abs() < 1e-6);
        assert_eq!(cfg.update_interval_ms, 250);
        assert_eq!(cfg.snapshots_per_cycle, 64);
        assert_eq!(cfg.spawns_per_cycle, 32);
        assert_eq!(cfg.updates_per_cycle, 33);
        assert_eq!(cfg.despawns_per_cycle, 34);
        assert_eq!(cfg.pins_per_cycle, 16);
        assert_eq!(cfg.unpins_per_cycle, 17);
        assert_eq!(cfg.max_cells_total, 1000);
        assert_eq!(cfg.interpolation_ticks, 2);
        assert_eq!(cfg.teleport_ticks, 3);
        assert_eq!(cfg.brightness, 12);
        assert_eq!(cfg.refresh_secs, 30);
        assert_eq!(cfg.tickets, TicketsBackend::ForceLoad);
        assert!(!cfg.tracking_raise);
        assert_eq!(cfg.log_interval_cycles, 5);
        assert_eq!(cfg.max_cells_per_player, 100);
    }

    #[test]
    fn inline_comments_and_whitespace() {
        let cfg = Config::parse("max_distance = 512   # far away\n\tbrightness=10\n").unwrap();
        assert_eq!(cfg.max_distance, 512);
        assert_eq!(cfg.brightness, 10);
    }

    #[test]
    fn rejects_unknown_key() {
        assert!(Config::parse("max_distant = 512").is_err());
    }

    #[test]
    fn rejects_duplicate_key() {
        assert!(Config::parse("growth = 2.0\ngrowth = 3.0").is_err());
    }

    #[test]
    fn rejects_bad_values() {
        assert!(Config::parse("max_distance = abc").is_err());
        assert!(Config::parse("max_distance = 100").is_err()); // < 128
        assert!(Config::parse("max_distance = 99999").is_err()); // > 32768
        assert!(Config::parse("growth = 1.0").is_err()); // out of range
        assert!(Config::parse("interpolation_ticks = 100").is_err());
        assert!(Config::parse("tickets = banana").is_err());
        assert!(Config::parse("tracking_raise = maybe").is_err());
        assert!(Config::parse("no_equals_here").is_err());
        assert!(Config::parse("brightness =").is_err());
    }
}
