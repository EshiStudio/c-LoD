//! Delta planner: desired cells vs active entities -> bounded op batches.
//!
//! Pure function of (desired, active, pinned, budgets) — no JVM, no I/O.
//! The engine feeds it every cycle and executes the returned ops on the
//! main thread within the per-cycle budgets.

use crate::quadtree::{CellKey, PlayerPos};
use std::collections::{HashMap, HashSet};

/// Render data for one LOD cell (block-space).
#[derive(Debug, Clone, PartialEq)]
pub struct CellVisual {
    /// Bottom Y of the impostor box (block coords).
    pub base_y: i32,
    /// Box height in blocks (>= 1).
    pub height: i32,
    /// Dominant map color of the cell top surface.
    pub color: [u8; 3],
    /// Representative block name (from the palette).
    pub material: String,
}

impl CellVisual {
    /// Visuals differing beyond these buckets are worth an entity update;
    /// smaller drifts are invisible at LOD distances and would churn metadata.
    pub fn stale(&self, other: &CellVisual) -> bool {
        (self.base_y - other.base_y).abs() > 4
            || (self.height - other.height).abs() > 4
            || color_dist(self.color, other.color) > 32 * 32 * 3
    }
}

fn color_dist(a: [u8; 3], b: [u8; 3]) -> i64 {
    let dr = a[0] as i64 - b[0] as i64;
    let dg = a[1] as i64 - b[1] as i64;
    let db = a[2] as i64 - b[2] as i64;
    dr * dr + dg * dg + db * db
}

/// A spawned display entity for a cell.
#[derive(Debug, Clone, PartialEq)]
pub struct ActiveEntity {
    pub uuid: String,
    pub visual: CellVisual,
}

/// One chunk pin/unpin target.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PinChunk {
    pub world: String,
    pub cx: i32,
    pub cz: i32,
}

/// Planned ops for one engine cycle (already budget-capped).
#[derive(Debug, Default, PartialEq)]
pub struct Plan {
    pub spawns: Vec<(CellKey, CellVisual)>,
    pub updates: Vec<(CellKey, String, CellVisual)>,
    pub despawns: Vec<(CellKey, String)>,
    pub pins: Vec<PinChunk>,
    pub unpins: Vec<PinChunk>,
    /// Level-0 cells with no cached chunk data yet (snapshot requests).
    pub need_snapshots: Vec<(CellKey, i32, i32)>,
}

/// Aggregation coverage below this fraction marks a multi-chunk cell as
/// data-less (grey box instead of half-guessed terrain).
const MIN_AGGREGATE_FRACTION: f32 = 0.5;

/// Aggregate a multi-chunk cell's visual from the chunk column cache.
///
/// `chunk_of(chunk_x, chunk_z) -> Option<Column>` is the worker's cache.
/// Returns `None` when fewer than `MIN_AGGREGATE_FRACTION` of the cell's
/// chunks have data.
pub fn aggregate_visual(
    key: &CellKey,
    mut chunk_of: impl FnMut(i32, i32) -> Option<crate::snapshot::Column>,
) -> Option<CellVisual> {
    let s = key.size();
    let need = (s * s) as f32;
    let mut have = 0f32;
    let mut min_y = i32::MAX;
    let mut max_y = i32::MIN;
    let mut color = [0f64; 3];
    for dx in 0..s {
        for dz in 0..s {
            if let Some(col) = chunk_of(key.cx + dx, key.cz + dz) {
                have += 1.0;
                min_y = min_y.min(col.min_y);
                max_y = max_y.max(col.max_y);
                for (i, c) in col.color.iter().enumerate() {
                    color[i] += *c as f64;
                }
            }
        }
    }
    if have / need < MIN_AGGREGATE_FRACTION {
        return None;
    }
    let base_y = (min_y - 1).max(-64);
    let height = (max_y - base_y + 1).clamp(1, 384 + 64 + 64);
    let c = [
        (color[0] / have as f64) as u8,
        (color[1] / have as f64) as u8,
        (color[2] / have as f64) as u8,
    ];
    Some(CellVisual {
        base_y,
        height,
        color: c,
        material: crate::palette::representative_block(c).to_string(),
    })
}

/// Plan one cycle.
///
/// * `desired` — cells from the quadtree pass, with their current visual
///   (`None` = not enough cached data yet).
/// * `active` — cells that currently have a display entity.
/// * `pinned` — chunks currently pinned by the module.
/// * `centers` — player chunk positions, nearest-first ordering reference.
pub fn plan_cycle(
    desired: &HashMap<CellKey, Option<CellVisual>>,
    active: &HashMap<CellKey, ActiveEntity>,
    pinned: &HashSet<PinChunk>,
    centers: &[PlayerPos],
    cfg: &crate::config::Config,
) -> Plan {
    let mut plan = Plan::default();

    // --- spawns & updates -----------------------------------------------
    let mut desired_pins: HashSet<PinChunk> = HashSet::new();
    let mut spawns_left = cfg.spawns_per_cycle as usize;
    let mut updates_left = cfg.updates_per_cycle as usize;
    let mut need_left = cfg.snapshots_per_cycle as usize;

    let mut desired_sorted: Vec<(&CellKey, &Option<CellVisual>)> = desired.iter().collect();
    desired_sorted.sort_by_cached_key(|(k, _)| cell_rank(k, centers));

    for (key, visual) in desired_sorted {
        // pin regardless of spawn budget (chunk liveness is cheap)
        desired_pins.insert(PinChunk {
            world: key.world.clone(),
            cx: key.cx,
            cz: key.cz,
        });
        match visual {
            Some(v) => match active.get(key) {
                None => {
                    if spawns_left > 0 {
                        plan.spawns.push((key.clone(), v.clone()));
                        spawns_left -= 1;
                    }
                }
                Some(ent) => {
                    if v.stale(&ent.visual) && updates_left > 0 {
                        plan.updates
                            .push((key.clone(), ent.uuid.clone(), v.clone()));
                        updates_left -= 1;
                    }
                }
            },
            None => {
                if key.level == 0 && need_left > 0 {
                    plan.need_snapshots.push((key.clone(), key.cx, key.cz));
                    need_left -= 1;
                }
            }
        }
    }

    // --- despawns ---------------------------------------------------------
    let mut despawns_left = cfg.despawns_per_cycle as usize;
    let mut missing: Vec<&CellKey> = active
        .keys()
        .filter(|k| !desired.contains_key(*k))
        .collect();
    missing.sort_by_cached_key(|k| cell_rank(k, centers));
    // farthest despawn first is nicer visually, but despawn budget pressure
    // says: nearest-first keeps the near horizon clean — use distance rank.
    for key in missing {
        if despawns_left == 0 {
            break;
        }
        if let Some(ent) = active.get(key) {
            plan.despawns.push((key.clone(), ent.uuid.clone()));
            despawns_left -= 1;
        }
    }

    // --- pins / unpins ------------------------------------------------------
    let mut pins_left = cfg.pins_per_cycle as usize;
    let mut unpins_left = cfg.unpins_per_cycle as usize;
    let mut new_pins: Vec<PinChunk> = desired_pins
        .iter()
        .filter(|p| !pinned.contains(*p))
        .cloned()
        .collect();
    new_pins.sort_by_cached_key(|p| chunk_rank(&p.world, p.cx, p.cz, centers));
    for p in new_pins {
        if pins_left == 0 {
            break;
        }
        plan.pins.push(p);
        pins_left -= 1;
    }
    let mut gone: Vec<&PinChunk> = pinned
        .iter()
        .filter(|p| !desired_pins.contains(*p))
        .collect();
    gone.sort_by_cached_key(|p| chunk_rank(&p.world, p.cx, p.cz, centers));
    for p in gone {
        if unpins_left == 0 {
            break;
        }
        plan.unpins.push((*p).clone());
        unpins_left -= 1;
    }

    plan
}

/// Rank: nearest players first, lower levels first (they pop in visibly).
fn cell_rank(key: &CellKey, centers: &[PlayerPos]) -> (u32, u32, i64) {
    let mut best = i64::MAX;
    for p in centers {
        let d = crate::quadtree::cell_center_distance(p.cx, p.cz, key.cx, key.cz, key.size());
        best = best.min((d * 1000.0) as i64);
    }
    (key.level as u32, 0, best)
}

fn chunk_rank(world: &str, cx: i32, cz: i32, centers: &[PlayerPos]) -> (u32, i64, u64) {
    let mut best = i64::MAX;
    for p in centers {
        let dx = (cx as f32 + 0.5) - p.cx;
        let dz = (cz as f32 + 0.5) - p.cz;
        let d = (dx * dx + dz * dz).sqrt();
        best = best.min((d * 1000.0) as i64);
    }
    // world name tiebreak for determinism
    let whash: u64 = world.bytes().fold(0xcbf29ce4u64, |h, b| {
        (h ^ b as u64).wrapping_mul(0x100000001b3)
    });
    (0, best, whash)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::Column;

    fn vis(base: i32, h: i32, c: [u8; 3]) -> CellVisual {
        CellVisual {
            base_y: base,
            height: h,
            color: c,
            material: "stone".into(),
        }
    }

    fn cfg() -> crate::config::Config {
        crate::config::Config::default()
    }

    fn key(level: u8, cx: i32, cz: i32) -> CellKey {
        CellKey {
            world: "w".into(),
            level,
            cx,
            cz,
        }
    }

    #[test]
    fn spawn_despawn_lifecycle() {
        let mut desired = HashMap::new();
        desired.insert(key(0, 16, 0), Some(vis(60, 20, [125; 3])));
        let active = HashMap::new();
        let plan = plan_cycle(&desired, &active, &HashSet::new(), &[], &cfg());
        assert_eq!(plan.spawns.len(), 1);
        assert_eq!(plan.spawns[0].0, key(0, 16, 0));
        assert_eq!(plan.pins.len(), 1);
        assert_eq!(plan.pins[0].cx, 16);
        assert!(plan.despawns.is_empty());

        // next cycle: entity active, nothing to do
        let mut active = HashMap::new();
        active.insert(
            key(0, 16, 0),
            ActiveEntity {
                uuid: "e1".into(),
                visual: vis(60, 20, [125; 3]),
            },
        );
        let pinned: HashSet<PinChunk> = plan.pins.iter().cloned().collect();
        let plan2 = plan_cycle(&desired, &active, &pinned, &[], &cfg());
        assert!(plan2.spawns.is_empty() && plan2.updates.is_empty());
        assert!(plan2.pins.is_empty() && plan2.unpins.is_empty());

        // removed from desired -> despawn + unpin
        let desired2 = HashMap::new();
        let plan3 = plan_cycle(&desired2, &active, &pinned, &[], &cfg());
        assert_eq!(plan3.despawns.len(), 1);
        assert_eq!(plan3.despawns[0].1, "e1");
        assert_eq!(plan3.unpins.len(), 1);
        assert_eq!(plan3.unpins[0].cx, 16);
    }

    #[test]
    fn update_on_stale_visual_only() {
        let k = key(1, 32, 32);
        let mut desired = HashMap::new();
        desired.insert(k.clone(), Some(vis(60, 20, [125; 3])));
        let mut active = HashMap::new();
        active.insert(
            k.clone(),
            ActiveEntity {
                uuid: "e".into(),
                visual: vis(60, 20, [125; 3]),
            },
        );
        let plan = plan_cycle(&desired, &active, &HashSet::new(), &[], &cfg());
        assert!(plan.updates.is_empty());

        // small drift -> no update
        desired.insert(k.clone(), Some(vis(62, 22, [130; 3])));
        let plan = plan_cycle(&desired, &active, &HashSet::new(), &[], &cfg());
        assert!(plan.updates.is_empty());

        // big drift -> update
        desired.insert(k.clone(), Some(vis(10, 90, [10, 10, 10])));
        let plan = plan_cycle(&desired, &active, &HashSet::new(), &[], &cfg());
        assert_eq!(plan.updates.len(), 1);
        assert_eq!(plan.updates[0].1, "e");
        assert_eq!(plan.updates[0].2.height, 90);
    }

    #[test]
    fn budgets_capped() {
        let mut desired = HashMap::new();
        for i in 0..100 {
            desired.insert(key(0, i * 2, 0), Some(vis(60, 20, [125; 3])));
        }
        let mut c = cfg();
        c.spawns_per_cycle = 5;
        c.pins_per_cycle = 3;
        let plan = plan_cycle(&desired, &HashMap::new(), &HashSet::new(), &[], &c);
        assert_eq!(plan.spawns.len(), 5);
        assert_eq!(plan.pins.len(), 3);
    }

    #[test]
    fn despawn_budget_capped_and_nearest_first() {
        let mut active = HashMap::new();
        for i in 0..10 {
            active.insert(
                key(0, i * 10, 0),
                ActiveEntity {
                    uuid: format!("e{}", i),
                    visual: vis(60, 20, [125; 3]),
                },
            );
        }
        let mut c = cfg();
        c.despawns_per_cycle = 4;
        // player near x=0: nearest cells despawn first
        let centers = vec![PlayerPos { cx: 1.0, cz: 0.5 }];
        let plan = plan_cycle(&HashMap::new(), &active, &HashSet::new(), &centers, &c);
        assert_eq!(plan.despawns.len(), 4);
        assert_eq!(plan.despawns[0].0, key(0, 0, 0));
        assert_eq!(plan.despawns[1].0, key(0, 10, 0));
    }

    #[test]
    fn level0_without_data_requests_snapshot() {
        let mut desired = HashMap::new();
        desired.insert(key(0, 16, 16), None);
        let plan = plan_cycle(&desired, &HashMap::new(), &HashSet::new(), &[], &cfg());
        assert_eq!(plan.need_snapshots.len(), 1);
        assert_eq!(plan.need_snapshots[0].1, 16);
        assert!(plan.spawns.is_empty());
    }

    #[test]
    fn level1_without_data_does_not_request_snapshot() {
        let mut desired = HashMap::new();
        desired.insert(key(1, 32, 32), None);
        let plan = plan_cycle(&desired, &HashMap::new(), &HashSet::new(), &[], &cfg());
        assert!(plan.need_snapshots.is_empty());
    }

    #[test]
    fn shared_pin_not_duplicated() {
        // level-0 cell at (32,32) and a level-2 cell at (32,32) share the
        // origin chunk — exactly one pin
        let mut desired = HashMap::new();
        desired.insert(key(0, 32, 32), Some(vis(60, 20, [125; 3])));
        desired.insert(key(2, 32, 32), Some(vis(60, 20, [125; 3])));
        let plan = plan_cycle(&desired, &HashMap::new(), &HashSet::new(), &[], &cfg());
        assert_eq!(plan.pins.len(), 1);
        assert_eq!(plan.spawns.len(), 2);
    }

    #[test]
    fn aggregate_visual_sums_children() {
        let k = key(1, 0, 0);
        let col = |h: i32| Column {
            min_y: h - 5,
            max_y: h,
            color: [100, 120, 80],
        };
        // all four children present
        let v = aggregate_visual(&k, |x, _z| Some(col(if x >= 1 { 70 } else { 60 })));
        let v = v.expect("full aggregate");
        assert_eq!(v.base_y, 54); // min(55,65)-1
        assert_eq!(v.height, 70 - 54 + 1);
        assert_eq!(v.color, [100, 120, 80]);

        // 25% coverage -> None
        let v = aggregate_visual(&k, |x, z| {
            if x == 0 && z == 0 {
                Some(col(60))
            } else {
                None
            }
        });
        assert!(v.is_none());
    }

    #[test]
    fn aggregate_material_from_color() {
        let k = key(0, 0, 0);
        let v = aggregate_visual(&k, |_x, _z| {
            Some(Column {
                min_y: 62,
                max_y: 68,
                color: crate::palette::map_color("grass_block"),
            })
        })
        .unwrap();
        assert_eq!(v.material, "grass_block");
    }

    #[test]
    fn plan_is_deterministic() {
        let mut desired = HashMap::new();
        for i in 0..30 {
            desired.insert(key(0, i * 3, -i), Some(vis(60, 20, [125; 3])));
        }
        let centers = vec![PlayerPos { cx: 0.0, cz: 0.0 }];
        let a = plan_cycle(&desired, &HashMap::new(), &HashSet::new(), &centers, &cfg());
        let b = plan_cycle(&desired, &HashMap::new(), &HashSet::new(), &centers, &cfg());
        assert_eq!(a, b);
    }
}
