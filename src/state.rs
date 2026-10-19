//! Global module state shared between the init thread, the worker thread and
//! main-thread jobs.

use crate::config::Config;
use std::sync::{Arc, OnceLock};

pub struct Shared {
    pub cfg: Config,
}

static SHARED: OnceLock<Arc<Shared>> = OnceLock::new();

/// Install the shared state once. Later calls are no-ops (hot reload safe).
pub fn install(cfg: Config) -> bool {
    let shared = Arc::new(Shared { cfg });
    SHARED.set(shared).is_ok()
}

pub fn get() -> Option<&'static Arc<Shared>> {
    SHARED.get()
}

pub fn cfg() -> Config {
    get().map(|s| s.cfg.clone()).unwrap_or_default()
}

/// Engine messaging between the worker thread and main-thread jobs.
pub mod msg {
    /// A player position snapshot read on the main thread.
    #[derive(Debug, Clone, PartialEq)]
    pub struct PlayerSnapshot {
        pub uuid: String,
        pub world: String,
        /// Chunk-space position (chunk = 16 blocks).
        pub cx: f32,
        pub cz: f32,
    }

    /// One decoded chunk column (256 top-block samples).
    #[derive(Debug, Clone)]
    pub struct ChunkData {
        pub world: String,
        pub cx: i32,
        pub cz: i32,
        /// (top_y, color) per block column, row-major x=0..16 outer.
        pub samples: Box<[(i32, [u8; 3]); 256]>,
        /// World min Y (for empty-column placeholders).
        pub min_world_y: i32,
    }

    /// Ops executed on the server main thread by the drain.
    #[derive(Debug, Clone)]
    pub enum JvmOp {
        ReadPlayers,
        ReadSnapshot {
            world: String,
            cx: i32,
            cz: i32,
        },
        Spawn {
            key: crate::quadtree::CellKey,
            visual: crate::plan::CellVisual,
        },
        Update {
            key: crate::quadtree::CellKey,
            uuid: String,
            visual: crate::plan::CellVisual,
        },
        Despawn {
            uuid: String,
        },
        Pin {
            world: String,
            cx: i32,
            cz: i32,
        },
        Unpin {
            world: String,
            cx: i32,
            cz: i32,
        },
    }

    /// Results flowing back to the worker.
    #[derive(Debug, Clone)]
    pub enum JvmResult {
        Players(Vec<PlayerSnapshot>),
        Snapshot(Option<ChunkData>),
        /// Entity vanished mid-cycle (worker respawns the cell next cycle).
        Dead(String),
        /// Spawn confirmed with the entity uuid.
        Spawned {
            key: crate::quadtree::CellKey,
            uuid: String,
        },
        /// Pin/unpin outcome (worker retries failures next cycle).
        PinAck {
            pin: bool,
            ok: bool,
            world: String,
            cx: i32,
            cz: i32,
        },
        /// Backend probe outcome (bootstrap only).
        Boot {
            view_distance: u32,
            note: String,
        },
    }
}
