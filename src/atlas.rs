//! The LOD heightmap atlas: a `cells x cells` grid covering a square world
//! region. Cell data:
//!   - height: i16 block Y (decoded from the atlas PNG's R/G channels),
//!   - water:  bool (sea surface instead of terrain).
//!
//! The atlas is filled incrementally from the main thread (JNI budget per
//! tick) and re-encoded into the resource pack when it changes.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock;

pub struct Atlas {
    /// Window center in world block coords; RwLock so the window can follow
    /// players via `recenter` while the atlas lives behind an Arc.
    center: RwLock<(i32, i32)>,
    pub size: i32,
    pub cell: i32,
    pub cells: u32,
    /// Kept for future y-band filtering; encoding uses it via cfg today.
    #[allow(dead_code)]
    pub sea_level: i32,
    data: RwLock<Vec<Cell>>,
    /// Monotonic geometry version: bumped whenever any cell changes.
    version: AtomicU64,
    /// Number of cells scanned at least once (progress tracking).
    scanned: AtomicU64,
}

#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub struct Cell {
    pub height: i16,
    pub water: bool,
    pub valid: bool,
}

impl Atlas {
    pub fn new(center_x: i32, center_z: i32, size: i32, cell: i32, sea_level: i32) -> Self {
        let cells = (size / cell).max(1) as u32;
        Self {
            center: RwLock::new((center_x, center_z)),
            size,
            cell,
            cells,
            sea_level,
            data: RwLock::new(vec![Cell::default(); (cells * cells) as usize]),
            version: AtomicU64::new(1),
            scanned: AtomicU64::new(0),
        }
    }

    /// Window center (world block coords).
    pub fn center(&self) -> (i32, i32) {
        *self.center.read().unwrap()
    }

    pub fn center_x(&self) -> i32 {
        self.center().0
    }

    pub fn center_z(&self) -> i32 {
        self.center().1
    }

    pub fn half(&self) -> i32 {
        self.size / 2
    }

    /// Cell indices for a world block coordinate, or None when outside.
    pub fn cell_index(&self, x: i32, z: i32) -> Option<(u32, u32)> {
        let (cx, cz) = self.center();
        let dx = x - (cx - self.half());
        let dz = z - (cz - self.half());
        if dx < 0 || dz < 0 || dx >= self.size || dz >= self.size {
            return None;
        }
        Some(((dx / self.cell) as u32, (dz / self.cell) as u32))
    }

    pub fn set(&self, ix: u32, iz: u32, height: i16, water: bool) {
        let idx = (iz * self.cells + ix) as usize;
        if let Ok(mut data) = self.data.write() {
            if let Some(cell) = data.get_mut(idx) {
                if !cell.valid || cell.height != height || cell.water != water {
                    if !cell.valid {
                        self.scanned.fetch_add(1, Ordering::Relaxed);
                    }
                    *cell = Cell {
                        height,
                        water,
                        valid: true,
                    };
                    self.version.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
    }

    #[allow(dead_code)] // used by tests and upcoming y-band filters
    pub fn get(&self, ix: u32, iz: u32) -> Option<Cell> {
        let idx = (iz * self.cells + ix) as usize;
        if let Ok(d) = self.data.read() {
            return d.get(idx).copied().filter(|c| c.valid);
        }
        None
    }

    pub fn version(&self) -> u64 {
        self.version.load(Ordering::Relaxed)
    }

    pub fn scanned_cells(&self) -> u64 {
        self.scanned.load(Ordering::Relaxed)
    }

    pub fn total_cells(&self) -> u64 {
        self.cells as u64 * self.cells as u64
    }

    /// Surface height at a world block position, when inside the window and
    /// scanned. Used by adaptive tiering and recenter decisions.
    pub fn height_at(&self, x: i32, z: i32) -> Option<i16> {
        let (ix, iz) = self.cell_index(x, z)?;
        self.get(ix, iz).map(|c| c.height)
    }

    /// Chebyshev distance a point lies OUTSIDE the atlas window (0 when
    /// inside). Used to decide when the window should follow the players.
    pub fn distance_outside(&self, x: i32, z: i32) -> i32 {
        let (cx, cz) = self.center();
        let lo_x = cx - self.half();
        let lo_z = cz - self.half();
        let dx = if x < lo_x {
            lo_x - x
        } else {
            (x - (lo_x + self.size - 1)).max(0)
        };
        let dz = if z < lo_z {
            lo_z - z
        } else {
            (z - (lo_z + self.size - 1)).max(0)
        };
        dx.max(dz)
    }

    /// Move the window to a new center and drop all cell data (version
    /// bumps, so the pack rebuilds and clients re-download). Thread-safe:
    /// may be called through the shared Arc from the main thread; the next
    /// scan sweep refills from scratch.
    pub fn recenter(&self, new_cx: i32, new_cz: i32) {
        {
            let mut c = self.center.write().unwrap();
            if *c == (new_cx, new_cz) {
                return;
            }
            *c = (new_cx, new_cz);
        }
        if let Ok(mut d) = self.data.write() {
            for cell in d.iter_mut() {
                *cell = Cell::default();
            }
        }
        self.scanned.store(0, Ordering::Relaxed);
        self.version.fetch_add(1, Ordering::Relaxed);
    }

    /// Snapshot the grid into a flat buffer (row-major, z outer).
    pub fn snapshot(&self) -> Vec<Cell> {
        self.data.read().map(|d| d.clone()).unwrap_or_default()
    }

    /// Iterate cell world-coords for a scan sweep.
    pub fn scan_task(&self, seq: u64, count: u32) -> Vec<(u32, u32)> {
        let cells = self.cells as u64;
        let total = cells * cells;
        if total == 0 {
            return Vec::new();
        }
        let mut out = Vec::with_capacity(count as usize);
        let mut s = seq % total;
        while out.len() < count as usize {
            out.push(((s % cells) as u32, (s / cells) as u32));
            s = (s + 1) % total;
            if out.len() as u64 == total {
                break;
            }
        }
        out
    }

    /// World X of cell center.
    pub fn cell_world_x(&self, ix: u32) -> i32 {
        self.center_x() - self.half() + (ix * self.cell as u32) as i32 + self.cell / 2
    }

    /// World Z of cell center.
    pub fn cell_world_z(&self, iz: u32) -> i32 {
        self.center_z() - self.half() + (iz * self.cell as u32) as i32 + self.cell / 2
    }

    /// Encode the atlas as RGBA8 pixels for the PNG texture:
    ///   R = high byte, G = low byte of `(height + 64) << 6`,
    ///   B = 255 when water, A = 255 (opaque).
    /// Unscanned cells encode height 0 / water so the shader treats them as
    /// void (below the culled minimum).
    pub fn encode_rgba(&self) -> (u32, u32, Vec<u8>) {
        let snap = self.snapshot();
        let n = self.cells as usize;
        let mut px = vec![0u8; n * n * 4];
        for z in 0..n {
            for x in 0..n {
                let cell = snap[z * n + x];
                let (y16, water) = if cell.valid {
                    let h = (i32::from(cell.height) + 64).max(0) << 6;
                    (h.min(0xFFFF) as u16, cell.water)
                } else {
                    (0, false)
                };
                let o = (z * n + x) * 4;
                px[o] = (y16 >> 8) as u8;
                px[o + 1] = (y16 & 0xFF) as u8;
                px[o + 2] = if water { 255 } else { 0 };
                px[o + 3] = 255;
            }
        }
        (self.cells, self.cells, px)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indices_and_bounds() {
        let a = Atlas::new(0, 0, 1024, 4, 63);
        assert_eq!(a.cells, 256);
        assert_eq!(a.half(), 512);
        assert_eq!(a.cell_index(0, 0), Some((128, 128)));
        assert_eq!(a.cell_index(-512, -512), Some((0, 0)));
        assert_eq!(a.cell_index(511, 511), Some((255, 255)));
        assert_eq!(a.cell_index(512, 0), None);
        assert_eq!(a.cell_index(-513, 0), None);
    }

    #[test]
    fn set_get_encode() {
        let a = Atlas::new(0, 0, 64, 4, 63);
        assert_eq!(a.cells, 16);
        a.set(3, 5, 100, false);
        a.set(0, 0, 62, true);
        assert_eq!(a.get(3, 5).unwrap().height, 100);
        assert!(a.get(0, 0).unwrap().water);
        assert!(a.get(1, 1).is_none());

        let (w, h, px) = a.encode_rgba();
        assert_eq!((w, h), (16, 16));
        // height 100 -> (100+64)<<6 = 10496 = 0x2900
        let o = (5 * 16 + 3) * 4;
        assert_eq!(px[o], 0x29);
        assert_eq!(px[o + 1], 0x00);
        // water cell at (0,0)
        let o2 = 0;
        assert_eq!(px[o2], ((62 + 64) << 6 >> 8) as u8);
        assert_eq!(px[o2 + 2], 255);
        // invalid -> zero
        let o3 = (7 * 16 + 7) * 4;
        assert_eq!(px[o3], 0);
        assert_eq!(px[o3 + 2], 0);
    }

    #[test]
    fn version_bumps_on_change() {
        let a = Atlas::new(0, 0, 64, 4, 63);
        let v0 = a.version();
        a.set(1, 1, 10, false);
        let v1 = a.version();
        a.set(1, 1, 10, false); // same value — no bump
        assert_eq!(v0 + 1, v1);
        assert_eq!(a.version(), v1);
        assert_eq!(a.scanned_cells(), 1);
    }

    #[test]
    fn scan_task_wraps() {
        let a = Atlas::new(0, 0, 64, 4, 63);
        // Asking for more than `total` yields exactly `total` unique cells.
        let t = a.scan_task(0, 257);
        assert_eq!(t.len(), 256);
        assert_eq!(t[0], (0, 0));
        assert_eq!(t[16], (0, 1));
        let t2 = a.scan_task(255, 4);
        assert_eq!(t2[0], (15, 15));
        assert_eq!(t2[1], (0, 0));
        assert_eq!(t2[2], (1, 0));
    }

    #[test]
    fn height_and_distance() {
        let a = Atlas::new(0, 0, 64, 4, 63);
        a.set(3, 5, 100, false);
        // Cell (3,5) covers world x in [-20, -17], z in [-12, -9].
        assert_eq!(a.height_at(-20, -12), Some(100));
        assert_eq!(a.height_at(-17, -9), Some(100));
        assert_eq!(a.height_at(0, 0), None); // valid=false
        assert_eq!(a.height_at(9999, 0), None);

        assert_eq!(a.distance_outside(0, 0), 0);
        assert_eq!(a.distance_outside(31, -10), 0);
        assert_eq!(a.distance_outside(40, 0), 9); // window is [-32, 31]
        assert_eq!(a.distance_outside(-50, 0), 18);
        assert_eq!(a.distance_outside(-50, 100), 69); // max(18, 69) Chebyshev
    }

    #[test]
    fn recenter_resets_and_bumps() {
        let a = Atlas::new(0, 0, 64, 4, 63);
        a.set(0, 0, 90, true);
        let v0 = a.version();
        a.recenter(1000, -2000);
        assert_eq!(a.center(), (1000, -2000));
        assert_eq!(a.scanned_cells(), 0);
        assert_eq!(a.version(), v0 + 1);
        assert_eq!(a.get(0, 0), None);
        // New window resolves around the new center.
        assert_eq!(a.distance_outside(1000, -2000), 0);
        assert!(a.distance_outside(0, 0) > 0);
        // No-op recenter keeps version.
        let v1 = a.version();
        a.recenter(1000, -2000);
        assert_eq!(a.version(), v1);
    }
}
