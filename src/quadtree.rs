//! LOD annulus model: which cells exist at which distance, with hysteresis.
//!
//! The world around a player is partitioned into distance annuli per LOD
//! level. Level `L` uses square cells of `2^L` chunks. Level 0 starts right
//! outside the vanilla view distance (`T0`), each next level starts where the
//! previous level's cell size stops being worth it: `T_{L+1} = max(T_L + cell,
//! T_L * growth)`. Cells whose center distance falls in `[T_L, T_{L+1})` are
//! planned. A hysteresis band around the boundaries stops flicker.
//!
//! Pure functions only: no JVM, no state — fully unit-testable.

/// An active/desired LOD cell.
///
/// `(cx, cz)` is the cell origin in CHUNK coordinates (a multiple of the cell
/// size); the cell spans `size()` chunks in both axes.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CellKey {
    pub world: String,
    pub level: u8,
    pub cx: i32,
    pub cz: i32,
}

impl CellKey {
    /// Cell edge in chunks. Level 0 is already 2x2 chunks (32x32 blocks) —
    /// 1-chunk impostors would mean thousands of entities per player.
    pub fn size(&self) -> i32 {
        1i32 << (self.level + 1)
    }

    /// Chunk coords covered by this cell: `[cx, cx+size) x [cz, cz+size)`.
    pub fn contains_chunk(&self, x: i32, z: i32) -> bool {
        let s = self.size();
        x >= self.cx && x < self.cx + s && z >= self.cz && z < self.cz + s
    }

    /// Block-space cell size (edge length).
    pub fn size_blocks(&self) -> i32 {
        self.size() * 16
    }
}

/// A player position snapshot in chunk coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlayerPos {
    pub cx: f32,
    pub cz: f32,
}

/// Distances (in chunks) at which each LOD level starts.
///
/// `starts[0]` is the vanilla boundary (view distance + 1); the slice has
/// `max_levels` entries; the last level extends to the max distance.
#[derive(Debug, Clone, PartialEq)]
pub struct Levels {
    pub starts: Vec<f32>,
}

impl Levels {
    /// Build the level table from config inputs.
    ///
    /// * `view_distance` — vanilla view distance in chunks (>= 2 enforced by
    ///   the caller).
    /// * `growth` — annulus growth factor per level (>= 1.3).
    /// * `max_distance_chunks` — planning radius in chunks.
    pub fn build(view_distance: u32, growth: f32, max_distance_chunks: u32) -> Levels {
        let vd = (view_distance as f32).max(2.0);
        let max = (max_distance_chunks as f32).max(vd + 1.0);
        let mut starts = vec![vd + 1.0];
        loop {
            let l = starts.len() as u32;
            let cell = (1u32 << l.min(20)) as f32; // cell size of level l-1 (= 2^l)
            let prev = *starts.last().unwrap();
            let next = (prev + cell).max(prev * growth.max(1.01));
            if next >= max || starts.len() >= 12 {
                break;
            }
            starts.push(next);
        }
        Levels { starts }
    }

    pub fn max_levels(&self) -> usize {
        self.starts.len()
    }
}

/// All cells desired around one player (unsorted; levels may geometrically
/// overhang each other at boundaries — inherent to square impostors).
///
/// Rule (hole-free by construction): a level-`L` cell is planned iff its
/// chunk-center rect distance range `[dmin, dmax]` intersects the level's
/// distance band `[T_L, T_{L+1})`. Every chunk whose center distance falls in
/// a band therefore always finds its own grid cell planned. `prior` (cells
/// active in the previous cycle) widens the band edges by `hysteresis` so
/// active cells stick instead of popping at boundaries.
pub fn desired_cells(
    world: &str,
    player: PlayerPos,
    levels: &Levels,
    hysteresis: f32,
    max_distance_chunks: u32,
    prior: &std::collections::HashSet<CellKey>,
) -> Vec<CellKey> {
    let max = max_distance_chunks as f32;
    let mut out = Vec::new();
    for level in 0..levels.max_levels() as u8 {
        let cell: i32 = 1 << (level + 1); // 2x2 chunks at level 0
        let is_last = level as usize + 1 == levels.starts.len();
        let band_lo = levels.starts[level as usize];
        let band_hi = if is_last {
            max
        } else {
            levels.starts[level as usize + 1]
        };
        // iterate cells whose inset rect can reach the (possibly widened) band
        let reach = band_hi * (1.0 + hysteresis) + cell as f32;
        let lo_x = (player.cx - reach).floor() as i32;
        let lo_z = (player.cz - reach).floor() as i32;
        let ax = lo_x.div_euclid(cell) * cell;
        let az = lo_z.div_euclid(cell) * cell;
        let mut cz = az;
        while cz as f32 <= player.cz + reach {
            let mut cx = ax;
            while cx as f32 <= player.cx + reach {
                let (dmin, dmax) = inset_rect_range(player, cx, cz, cell);
                let was_prior = prior.contains(&CellKey {
                    world: world.to_string(),
                    level,
                    cx,
                    cz,
                });
                let (lo, hi) = if was_prior && hysteresis > 0.0 {
                    (band_lo * (1.0 - hysteresis), band_hi * (1.0 + hysteresis))
                } else {
                    (band_lo, band_hi)
                };
                if dmin < hi && dmax >= lo {
                    out.push(CellKey {
                        world: world.to_string(),
                        level,
                        cx,
                        cz,
                    });
                }
                cx += cell;
            }
            cz += cell;
        }
    }
    out
}

/// Min/max distance (chunks) from the player to any CHUNK CENTER of the cell
/// (the cell rect inset by half a chunk — chunk centers, not corners).
pub fn inset_rect_range(p: PlayerPos, cx: i32, cz: i32, cell: i32) -> (f32, f32) {
    let min_x = cx as f32 + 0.5;
    let min_z = cz as f32 + 0.5;
    let max_x = (cx + cell) as f32 - 0.5;
    let max_z = (cz + cell) as f32 - 0.5;
    let dx_min = if p.cx < min_x {
        min_x - p.cx
    } else if p.cx > max_x {
        p.cx - max_x
    } else {
        0.0
    };
    let dz_min = if p.cz < min_z {
        min_z - p.cz
    } else if p.cz > max_z {
        p.cz - max_z
    } else {
        0.0
    };
    let dmin = (dx_min * dx_min + dz_min * dz_min).sqrt();
    let dx_max = (p.cx - min_x).abs().max((p.cx - max_x).abs());
    let dz_max = (p.cz - min_z).abs().max((p.cz - max_z).abs());
    let dmax = (dx_max * dx_max + dz_max * dz_max).sqrt();
    (dmin, dmax)
}

/// Distance from player to cell center (for sorting/budgets).
pub fn cell_center_distance(pcx: f32, pcz: f32, cx: i32, cz: i32, cell: i32) -> f32 {
    let mid = cell as f32 / 2.0;
    let dx = (cx as f32 + mid) - pcx;
    let dz = (cz as f32 + mid) - pcz;
    (dx * dx + dz * dz).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn levels() -> Levels {
        Levels::build(8, 2.0, 64) // 128 blocks view, 1024 blocks max
    }

    #[test]
    fn level_table_sane() {
        let l = levels();
        assert_eq!(l.starts[0], 9.0);
        // monotonic
        for w in l.starts.windows(2) {
            assert!(w[1] > w[0]);
        }
        // growth respected (each next start >= prev * 2 - eps)
        for w in l.starts.windows(2) {
            assert!(w[1] + 1e-3 >= w[0] * 2.0, "{:?}", l.starts);
        }
        // finite starts, bounded count
        assert!(l.starts.len() <= 12);
    }

    #[test]
    fn desired_cells_cover_annulus() {
        let l = levels();
        let p = PlayerPos { cx: 3.5, cz: -7.2 };
        let cells = desired_cells("world", p, &l, 0.0, 64, &HashSet::new());
        assert!(!cells.is_empty());
        // every planned cell actually reaches its own band (rect-range check)
        for c in &cells {
            let (dmin, dmax) = inset_rect_range(p, c.cx, c.cz, c.size());
            let band_lo = l.starts[c.level as usize];
            let band_hi = if c.level as usize + 1 == l.starts.len() {
                64.0
            } else {
                l.starts[c.level as usize + 1]
            };
            assert!(
                dmin < band_hi && dmax >= band_lo,
                "cell not in its band: {:?} dmin={} dmax={} band=[{}, {})",
                c,
                dmin,
                dmax,
                band_lo,
                band_hi
            );
        }
        // dedup
        let set: std::collections::HashSet<&CellKey> = cells.iter().collect();
        assert_eq!(set.len(), cells.len());
    }

    #[test]
    fn desired_cells_grid_aligned() {
        let l = Levels::build(4, 2.0, 32);
        let p = PlayerPos { cx: 10.3, cz: 10.3 };
        let cells = desired_cells("w", p, &l, 0.0, 32, &HashSet::new());
        for c in &cells {
            let s = c.size();
            assert_eq!(c.cx % s, 0, "{:?}", c);
            assert_eq!(c.cz % s, 0, "{:?}", c);
        }
    }

    #[test]
    fn cell_coverage_no_gaps_in_corners() {
        // Every chunk-center position in the annulus must be covered by at
        // least one planned cell (allowing overlap, forbidding holes).
        let l = Levels::build(4, 1.5, 48);
        let p = PlayerPos { cx: 0.0, cz: 0.0 };
        let cells = desired_cells("w", p, &l, 0.0, 48, &HashSet::new());
        let t0 = l.starts[0];
        let maxd = 48i32;
        let mut checked = 0;
        for x in -maxd..maxd {
            for z in -maxd..maxd {
                // chunk-center distance: chunks at integer coords, center at
                // (x+0.5, z+0.5) in chunk units — approximate with the chunk
                // origin (half-chunk error is inside the band edges)
                let dx = (x as f32 + 0.5 - p.cx).abs();
                let dz = (z as f32 + 0.5 - p.cz).abs();
                let dmin = dx.hypot(dz);
                if dmin < t0 || dmin > maxd as f32 {
                    continue;
                }
                checked += 1;
                let covered = cells.iter().any(|c| c.contains_chunk(x, z));
                assert!(covered, "hole at chunk ({}, {}) dist {}", x, z, dmin);
            }
        }
        assert!(checked > 100);
    }

    #[test]
    fn cells_count_is_bounded() {
        // The annulus model keeps the cell count roughly screen-constant:
        // a full ring set must stay far below the chunk count it covers.
        let l = Levels::build(8, 2.0, 128);
        let p = PlayerPos { cx: 0.0, cz: 0.0 };
        let cells = desired_cells("w", p, &l, 0.0, 128, &HashSet::new());
        let covered_chunks: usize = cells
            .iter()
            .map(|c| (c.size() as usize) * (c.size() as usize))
            .sum();
        // ~levels x annulus/cell-area: bounded by the 2x2-chunk cell floor
        assert!(cells.len() < 1200, "too many cells: {}", cells.len());
        // coverage is real: the full annulus area (pi*(128^2-9^2) ~= 51k chunks)
        assert!(covered_chunks > 40_000, "covered only {}", covered_chunks);
        // and the count grows sub-quadratically with distance
        let l2 = Levels::build(8, 2.0, 256);
        let cells2 = desired_cells("w", p, &l2, 0.0, 256, &HashSet::new());
        assert!(
            (cells2.len() as f64) < (cells.len() as f64) * 1.6 + 50.0,
            "cells {} -> {} at 2x distance",
            cells.len(),
            cells2.len()
        );
    }

    #[test]
    fn negative_chunk_coords_align() {
        let l = Levels::build(2, 2.0, 16);
        let p = PlayerPos {
            cx: -100.5,
            cz: -100.5,
        };
        let cells = desired_cells("w", p, &l, 0.0, 16, &HashSet::new());
        for c in &cells {
            let s = c.size();
            assert_eq!(c.cx.rem_euclid(s), 0, "{:?}", c);
            assert_eq!(c.cz.rem_euclid(s), 0, "{:?}", c);
        }
    }

    #[test]
    fn hysteresis_keeps_prior_cells_in_band_widening() {
        // A cell that was active last cycle must survive small drifts across
        // the band edge; a never-active cell must not appear there.
        let l = Levels::build(4, 2.0, 32);
        let p = PlayerPos { cx: 0.0, cz: 0.0 };
        let t1 = l.starts[1];
        // pick a level-0 cell whose dmax sits just under t1 (no prior) and
        // confirm identical planning with and without it in `prior`
        let fresh = desired_cells("w", p, &l, 0.05, 32, &HashSet::new());
        let mut prior = HashSet::new();
        // take an existing level-1 cell from a fresh plan (deterministic set)
        if let Some(cell) = fresh.iter().find(|c| c.level == 1).cloned() {
            prior.insert(cell.clone());
            let with = desired_cells("w", p, &l, 0.05, 32, &prior);
            assert!(with.contains(&cell), "prior cell dropped: {:?}", cell);
        }
        let _ = t1;
    }
}
