//! The LOD heightmap atlas: a `cells x cells` grid covering a square world
//! region. Cell data:
//!   - height: i16 block Y (decoded from the atlas PNG's R/G channels),
//!   - water:  bool (sea surface instead of terrain).
//!
//! Static-pack lifecycle (v3): the atlas is filled ONCE (JNI budget per tick
//! until completion or the scan deadline), optionally persisted to
//! `c-LoD-atlas.bin`, encoded into the resource pack exactly once, and never
//! mutated again. Restarts restore the snapshot so the pack SHA-1 — and the
//! client download state — stays identical across reboots.

use std::io::Write as _;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock;

/// On-disk snapshot magic + format version.
const MAGIC: &[u8; 8] = b"CLODAT01";

pub struct Atlas {
    pub center_x: i32,
    pub center_z: i32,
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

#[derive(Clone, Copy, Default, PartialEq)]
pub struct Cell {
    pub height: i16,
    pub water: bool,
    pub valid: bool,
}

impl Atlas {
    pub fn new(center_x: i32, center_z: i32, size: i32, cell: i32, sea_level: i32) -> Self {
        let cells = (size / cell).max(1) as u32;
        Self {
            center_x,
            center_z,
            size,
            cell,
            cells,
            sea_level,
            data: RwLock::new(vec![Cell::default(); (cells * cells) as usize]),
            version: AtomicU64::new(1),
            scanned: AtomicU64::new(0),
        }
    }

    pub fn half(&self) -> i32 {
        self.size / 2
    }

    /// Cell indices for a world block coordinate, or None when outside.
    pub fn cell_index(&self, x: i32, z: i32) -> Option<(u32, u32)> {
        let dx = x - (self.center_x - self.half());
        let dz = z - (self.center_z - self.half());
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
        self.center_x - self.half() + (ix * self.cell as u32) as i32 + self.cell / 2
    }

    /// World Z of cell center.
    pub fn cell_world_z(&self, iz: u32) -> i32 {
        self.center_z - self.half() + (iz * self.cell as u32) as i32 + self.cell / 2
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

    // -----------------------------------------------------------------------
    // Disk persistence (static-pack determinism across restarts).
    // -----------------------------------------------------------------------

    /// Serialize the atlas snapshot: MAGIC + header + 3 bytes per cell
    /// (height i16 LE, flags: bit0 water, bit1 valid).
    pub fn to_bytes(&self) -> Vec<u8> {
        let snap = self.snapshot();
        let n = snap.len();
        let mut out = Vec::with_capacity(MAGIC.len() + 20 + n * 3);
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&self.center_x.to_le_bytes());
        out.extend_from_slice(&self.center_z.to_le_bytes());
        out.extend_from_slice(&self.size.to_le_bytes());
        out.extend_from_slice(&self.cell.to_le_bytes());
        out.extend_from_slice(&self.sea_level.to_le_bytes());
        for c in &snap {
            out.extend_from_slice(&c.height.to_le_bytes());
            out.push((u8::from(c.water)) | (u8::from(c.valid) << 1));
        }
        out
    }

    /// Parse a snapshot produced by [`Self::to_bytes`]. Returns None on any
    /// header/geometry mismatch (stale cache for another world or config).
    pub fn from_bytes(
        buf: &[u8],
        want_center_x: i32,
        want_center_z: i32,
        want_size: i32,
        want_cell: i32,
        want_sea_level: i32,
    ) -> Option<Atlas> {
        if buf.len() < MAGIC.len() + 20 || &buf[..8] != MAGIC {
            return None;
        }
        let rd_i32 = |o: usize| -> Option<i32> {
            Some(i32::from_le_bytes(buf.get(o..o + 4)?.try_into().ok()?))
        };
        let cx = rd_i32(8)?;
        let cz = rd_i32(12)?;
        let size = rd_i32(16)?;
        let cell = rd_i32(20)?;
        let sea = rd_i32(24)?;
        if cx != want_center_x
            || cz != want_center_z
            || size != want_size
            || cell != want_cell
            || sea != want_sea_level
        {
            return None;
        }
        let atlas = Atlas::new(cx, cz, size, cell, sea);
        let n = (atlas.cells as usize) * (atlas.cells as usize);
        if buf.len() < MAGIC.len() + 20 + n * 3 {
            return None;
        }
        let mut data = atlas.data.write().ok()?;
        let base = MAGIC.len() + 20;
        let mut scanned = 0u64;
        for (i, cell_out) in data.iter_mut().enumerate() {
            let o = base + i * 3;
            let height = i16::from_le_bytes([buf[o], buf[o + 1]]);
            let flags = buf[o + 2];
            if flags & 0b10 != 0 {
                *cell_out = Cell {
                    height,
                    water: flags & 0b01 != 0,
                    valid: true,
                };
                scanned += 1;
            }
        }
        drop(data);
        atlas.scanned.store(scanned, Ordering::Relaxed);
        Some(atlas)
    }

    /// Atomically persist the snapshot (tmp file + rename).
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let bytes = self.to_bytes();
        let tmp = path.with_extension("tmp");
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        {
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(&bytes)?;
            f.sync_all()?;
        }
        std::fs::rename(&tmp, path)
    }

    /// Load a snapshot if it matches the expected geometry, else None.
    pub fn try_restore(
        path: &Path,
        center_x: i32,
        center_z: i32,
        size: i32,
        cell: i32,
        sea_level: i32,
    ) -> Option<Atlas> {
        let buf = std::fs::read(path).ok()?;
        Self::from_bytes(&buf, center_x, center_z, size, cell, sea_level)
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
    fn save_restore_roundtrip() {
        let dir = std::env::temp_dir().join(format!("clod-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("atlas.bin");

        let a = Atlas::new(123, -456, 64, 4, 62);
        a.set(3, 5, 100, false);
        a.set(0, 0, 62, true);
        a.set(15, 15, 200, false);
        a.save(&path).unwrap();
        // tmp file must be gone after atomic rename
        assert!(!path.with_extension("tmp").exists());

        let b = Atlas::try_restore(&path, 123, -456, 64, 4, 62).expect("restore");
        assert_eq!(b.center_x, 123);
        assert_eq!(b.center_z, -456);
        assert_eq!(b.scanned_cells(), 3);
        assert_eq!(b.get(3, 5).unwrap().height, 100);
        assert!(b.get(0, 0).unwrap().water);
        assert_eq!(b.get(15, 15).unwrap().height, 200);

        // Deterministic encoding across instances (pack SHA-1 stability).
        let (_, _, px_a) = a.encode_rgba();
        let (_, _, px_b) = b.encode_rgba();
        assert_eq!(px_a, px_b);

        // Geometry mismatch -> no restore (stale cache).
        assert!(Atlas::try_restore(&path, 999, -456, 64, 4, 62).is_none());
        assert!(Atlas::try_restore(&path, 123, -456, 128, 4, 62).is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn from_bytes_rejects_garbage() {
        assert!(Atlas::from_bytes(b"", 0, 0, 64, 4, 63).is_none());
        assert!(Atlas::from_bytes(b"NOTMAGIxxxx", 0, 0, 64, 4, 63).is_none());
        let a = Atlas::new(0, 0, 64, 4, 63);
        let bytes = a.to_bytes();
        let mut truncated = bytes.clone();
        truncated.truncate(bytes.len() - 7);
        assert!(Atlas::from_bytes(&truncated, 0, 0, 64, 4, 63).is_none());
    }
}
