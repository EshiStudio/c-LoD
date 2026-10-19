//! Snapshot model: one cached chunk column as seen by the LOD engine.

/// Aggregated visual data of one CHUNK column (16x16 blocks).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Column {
    /// Lowest terrain Y seen in the column.
    pub min_y: i32,
    /// Highest non-air Y seen in the column (surface top).
    pub max_y: i32,
    /// Average map color over the column's top blocks.
    pub color: [u8; 3],
}

impl Column {
    /// Build a column from per-column-block (height, color) samples.
    ///
    /// `samples[x][z] = (top_y, color)`, ordered `[x][z]` with x/z in 0..16.
    /// Empty columns (all void) produce a flat sea-level-ish placeholder so
    /// aggregates stay stable.
    pub fn from_samples(samples: &[(i32, [u8; 3]); 256], min_world_y: i32) -> Column {
        let mut min_y = i32::MAX;
        let mut max_y = i32::MIN;
        let mut acc = [0u64; 3];
        for (y, c) in samples.iter() {
            if *y < min_y {
                min_y = *y;
            }
            if *y > max_y {
                max_y = *y;
            }
            for i in 0..3 {
                acc[i] += c[i] as u64;
            }
        }
        if max_y == i32::MIN {
            // completely empty chunk
            return Column {
                min_y: min_world_y,
                max_y: min_world_y,
                color: crate::palette::FALLBACK,
            };
        }
        Column {
            min_y,
            max_y,
            color: [
                (acc[0] / 256) as u8,
                (acc[1] / 256) as u8,
                (acc[2] / 256) as u8,
            ],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(y: i32, c: [u8; 3]) -> (i32, [u8; 3]) {
        (y, c)
    }

    #[test]
    fn flat_world_column() {
        let samples: Vec<(i32, [u8; 3])> = (0..256).map(|_| flat(64, [125, 125, 125])).collect();
        let mut arr = [(0i32, [0u8; 3]); 256];
        for (i, s) in samples.into_iter().enumerate() {
            arr[i] = s;
        }
        let col = Column::from_samples(&arr, -64);
        assert_eq!(col.min_y, 64);
        assert_eq!(col.max_y, 64);
        assert_eq!(col.color, [125, 125, 125]);
    }

    #[test]
    fn mixed_heights_and_colors() {
        let mut arr = [(0i32, [0u8; 3]); 256];
        for (i, s) in arr.iter_mut().enumerate() {
            if i % 2 == 0 {
                *s = (70, [0, 0, 255]);
            } else {
                *s = (90, [255, 255, 255]);
            }
        }
        let col = Column::from_samples(&arr, -64);
        assert_eq!(col.min_y, 70);
        assert_eq!(col.max_y, 90);
        assert_eq!(col.color, [127, 127, 255]);
    }

    #[test]
    fn empty_column_placeholder() {
        let arr = [(i32::MIN, [0u8; 3]); 256];
        let col = Column::from_samples(&arr, -64);
        assert_eq!(col.min_y, -64);
        assert_eq!(col.max_y, -64);
        assert_eq!(col.color, crate::palette::FALLBACK);
    }
}
