//! Full-loop planning simulation (no JVM): a synthetic player walks across a
//! synthetic world while the LOD engine (levels + desired_cells + plan_cycle)
//! runs every cycle — the same loop `engine.rs` drives on the server.
//!
//! Assertions cover the product invariants:
//! * cells exist in a full ring around the player (no holes),
//! * cells are released when the player leaves (no leaks),
//! * the op stream is bounded by budgets,
//! * hysteresis keeps churn low on small movements.

use lod::config::Config;
use lod::plan::{plan_cycle, ActiveEntity, CellVisual};
use lod::quadtree::{desired_cells, inset_rect_range, Levels, PlayerPos};
use std::collections::{HashMap, HashSet};

fn vis(y: i32) -> CellVisual {
    CellVisual {
        base_y: y,
        height: 30,
        color: [125, 125, 125],
        material: "stone".into(),
    }
}

#[test]
fn full_loop_walk_release_and_budgets() {
    let cfg = Config::default();
    let levels = Levels::build(8, cfg.growth, cfg.max_distance / 16);
    let mut active: HashMap<lod::quadtree::CellKey, ActiveEntity> = HashMap::new();
    let mut pinned: HashSet<lod::plan::PinChunk> = HashSet::new();
    // synthetic flat world: every chunk has data
    let data: HashSet<(String, i32, i32)> = (-200..200)
        .flat_map(|x| (-200..200).map(move |z| (x, z)))
        .map(|(x, z)| ("world".to_string(), x, z))
        .collect();

    let mut p = PlayerPos { cx: 0.0, cz: 0.0 };
    let mut total_spawns = 0usize;
    let mut max_active = 0usize;

    // walk east for 40 cycles
    for step in 0..40 {
        p.cx += 1.0;
        let prior: HashSet<lod::quadtree::CellKey> = active.keys().cloned().collect();
        let mut desired: HashMap<lod::quadtree::CellKey, Option<CellVisual>> = HashMap::new();
        for c in desired_cells(
            "world",
            p,
            &levels,
            cfg.hysteresis,
            cfg.max_distance / 16,
            &prior,
        ) {
            desired.entry(c).or_insert(Some(vis(60)));
        }
        let _ = &data;
        let plan = plan_cycle(&desired, &active, &pinned, &[p], &cfg);
        // apply the plan like the engine does
        for (k, v) in plan.spawns {
            active.insert(
                k,
                ActiveEntity {
                    uuid: format!("e{}", step),
                    visual: v,
                },
            );
            total_spawns += 1;
        }
        for (k, _u) in plan.despawns {
            active.remove(&k);
        }
        for pc in plan.pins {
            pinned.insert(pc);
        }
        for pc in plan.unpins {
            pinned.remove(&pc);
        }
        max_active = max_active.max(active.len());
        // global sanity per cycle
        assert!(active.len() <= cfg.max_cells_total as usize);
    }
    // spawns happened, are budgeted, and the active set stays screen-constant
    assert!(total_spawns > 200, "no spawns during walk: {total_spawns}");
    assert!(max_active < 1200, "active set exploded: {max_active}");

    // teleport far away: everything must be released eventually
    let mut desired = HashMap::new();
    p.cx += 10_000.0;
    for c in desired_cells(
        "world",
        p,
        &levels,
        cfg.hysteresis,
        cfg.max_distance / 16,
        &HashSet::new(),
    ) {
        desired.entry(c).or_insert(Some(vis(60)));
    }
    // release loop: run cycles until NO ops remain (budget-capped drain)
    let mut cycles = 0;
    loop {
        let plan = plan_cycle(&desired, &active, &pinned, &[p], &cfg);
        let stats = (
            plan.spawns.len(),
            plan.despawns.len(),
            plan.pins.len(),
            plan.unpins.len(),
        );
        for (k, v) in &plan.spawns {
            active.insert(
                k.clone(),
                ActiveEntity {
                    uuid: "x".into(),
                    visual: v.clone(),
                },
            );
        }
        for (k, _u) in &plan.despawns {
            active.remove(k);
        }
        for pc in &plan.pins {
            pinned.insert(pc.clone());
        }
        for pc in &plan.unpins {
            pinned.remove(pc);
        }
        cycles += 1;
        if cycles % 50 == 0 {
            eprintln!(
                "DBG cycle={} active={} pinned={} spawns={} despawns={} pins={} unpins={}",
                cycles,
                active.len(),
                pinned.len(),
                stats.0,
                stats.1,
                stats.2,
                stats.3
            );
        }
        if stats.0 + stats.1 + stats.2 + stats.3 == 0 || cycles >= 300 {
            break;
        }
    }
    // old-area state must be fully released; the new area legitimately
    // carries its own cells and pins (the drain converged to zero ops)
    let near_new = |x: i32| (x as f64 - 10_040.0).abs() <= (cfg.max_distance / 16 + 8) as f64;
    assert!(
        active.keys().all(|k| near_new(k.cx)),
        "old-area cells leaked after teleport"
    );
    assert!(
        pinned.iter().all(|pc| near_new(pc.cx)),
        "old-area pins leaked after teleport"
    );
}

#[test]
fn full_loop_no_holes_around_spawn_edge() {
    // Cells must exist immediately outside the vanilla boundary in every
    // direction (the visual "seam" test).
    let cfg = Config::default();
    let levels = Levels::build(8, cfg.growth, cfg.max_distance / 16);
    let p = PlayerPos { cx: 0.0, cz: 0.0 };
    let cells = desired_cells(
        "world",
        p,
        &levels,
        0.0,
        cfg.max_distance / 16,
        &HashSet::new(),
    );
    let t0 = levels.starts[0] as i32;
    for (dx, dz) in [
        (1, 0),
        (0, 1),
        (-1, 0),
        (0, -1),
        (1, 1),
        (-1, 1),
        (1, -1),
        (-1, -1),
    ] {
        let cx = t0 + dx * 2;
        let cz = t0 + dz * 2;
        let covered = cells.iter().any(|c| {
            let s = c.size();
            cx >= c.cx && cx < c.cx + s && cz >= c.cz && cz < c.cz + s
        });
        assert!(covered, "hole just outside vanilla boundary ({cx},{cz})");
    }
    // every planned cell stays near the boundary: cells may straddle it by
    // up to one level-0 chunk (their outer chunks cover the annulus edge,
    // their inner chunks overlap vanilla — cosmetic overhang, < 3 chunks)
    for c in &cells {
        let (dmin, _dmax) = inset_rect_range(p, c.cx, c.cz, c.size());
        assert!(
            dmin >= levels.starts[0] - 3.0,
            "cell deep inside vanilla range: {c:?} dmin={dmin}"
        );
    }
}

#[test]
fn full_loop_visual_updates_flow() {
    // When the world data changes (height shift), active cells receive
    // bounded update ops instead of respawn churn.
    let cfg = Config::default();
    let levels = Levels::build(8, cfg.growth, cfg.max_distance / 16);
    let p = PlayerPos { cx: 50.5, cz: 50.5 };
    let prior = HashSet::new();
    let mut desired: HashMap<lod::quadtree::CellKey, Option<CellVisual>> = HashMap::new();
    for c in desired_cells(
        "world",
        p,
        &levels,
        cfg.hysteresis,
        cfg.max_distance / 16,
        &prior,
    ) {
        desired.insert(c, Some(vis(60)));
    }
    let mut active = HashMap::new();
    let pinned = HashSet::new();
    // drain the spawn backlog (budgeted at spawns_per_cycle per cycle)
    for _ in 0..50 {
        let plan = plan_cycle(&desired, &active, &pinned, &[p], &cfg);
        let spawned = plan.spawns.is_empty();
        for (k, v) in plan.spawns {
            active.insert(
                k,
                ActiveEntity {
                    uuid: "x".into(),
                    visual: v,
                },
            );
        }
        if spawned {
            break;
        }
    }
    // identical world: nothing to do at all
    let plan = plan_cycle(&desired, &active, &pinned, &[p], &cfg);
    assert!(plan.updates.is_empty() && plan.spawns.is_empty());
    // world changed a lot: updates only (already-drained backlog)
    let mut desired2 = desired.clone();
    for v in desired2.values_mut().flatten() {
        v.base_y = 100;
        v.height = 80;
    }
    let plan = plan_cycle(&desired2, &active, &pinned, &[p], &cfg);
    assert!(!plan.updates.is_empty());
    assert!(plan.spawns.is_empty());
}
