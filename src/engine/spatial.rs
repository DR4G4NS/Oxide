#![allow(dead_code)]

use parking_lot::RwLock;
use std::sync::Arc;

pub const CHUNK_SIZE: usize = 16;

#[derive(Clone)]
pub struct SpatialChunk {
    pub x: i32,
    pub y: i32,
    pub active_entities: Vec<u32>,
}

pub struct SpatialHashGrid {
    pub width_chunks: usize,
    pub height_chunks: usize,
    pub chunks: Vec<Arc<RwLock<SpatialChunk>>>,
}

impl SpatialHashGrid {
    pub fn new(width_tiles: usize, height_tiles: usize) -> Self {
        let width_chunks = width_tiles.div_ceil(CHUNK_SIZE);
        let height_chunks = height_tiles.div_ceil(CHUNK_SIZE);
        let total = width_chunks * height_chunks;

        let mut chunks = Vec::with_capacity(total);
        for cy in 0..height_chunks {
            for cx in 0..width_chunks {
                chunks.push(Arc::new(RwLock::new(SpatialChunk {
                    x: cx as i32,
                    y: cy as i32,
                    active_entities: Vec::new(),
                })));
            }
        }

        Self {
            width_chunks,
            height_chunks,
            chunks,
        }
    }

    pub fn get_chunk(&self, cx: usize, cy: usize) -> Option<&Arc<RwLock<SpatialChunk>>> {
        if cx < self.width_chunks && cy < self.height_chunks {
            Some(&self.chunks[cx + cy * self.width_chunks])
        } else {
            None
        }
    }

    /// World-space DDA over 8-px tiles (Arc `Intersector` + tile iteration).
    pub fn tiles_along_segment(x0: f32, y0: f32, x1: f32, y1: f32) -> Vec<(i16, i16)> {
        tiles_along_segment(x0, y0, x1, y1)
    }
}

/// Grid traversal used by bullet/building collision (audit M23).
pub fn tiles_along_segment(x0: f32, y0: f32, x1: f32, y1: f32) -> Vec<(i16, i16)> {
    let mut tile_x = (x0 / 8.0).floor() as i32;
    let mut tile_y = (y0 / 8.0).floor() as i32;
    let end_x = (x1 / 8.0).floor() as i32;
    let end_y = (y1 / 8.0).floor() as i32;
    let mut tiles = vec![(tile_x as i16, tile_y as i16)];
    if tile_x == end_x && tile_y == end_y {
        return tiles;
    }
    let dx = x1 - x0;
    let dy = y1 - y0;
    let step_x = if dx > 0.0 {
        1
    } else if dx < 0.0 {
        -1
    } else {
        0
    };
    let step_y = if dy > 0.0 {
        1
    } else if dy < 0.0 {
        -1
    } else {
        0
    };
    let next_boundary_x = if step_x > 0 {
        (tile_x as f32 + 1.0) * 8.0
    } else {
        tile_x as f32 * 8.0
    };
    let next_boundary_y = if step_y > 0 {
        (tile_y as f32 + 1.0) * 8.0
    } else {
        tile_y as f32 * 8.0
    };
    let mut t_max_x = if step_x == 0 {
        f32::INFINITY
    } else {
        (next_boundary_x - x0) / dx
    };
    let mut t_max_y = if step_y == 0 {
        f32::INFINITY
    } else {
        (next_boundary_y - y0) / dy
    };
    let t_delta_x = if step_x == 0 {
        f32::INFINITY
    } else {
        8.0 / dx.abs()
    };
    let t_delta_y = if step_y == 0 {
        f32::INFINITY
    } else {
        8.0 / dy.abs()
    };
    for _ in 0..4096 {
        if t_max_x < t_max_y {
            tile_x += step_x;
            t_max_x += t_delta_x;
        } else {
            tile_y += step_y;
            t_max_y += t_delta_y;
        }
        tiles.push((tile_x as i16, tile_y as i16));
        if tile_x == end_x && tile_y == end_y {
            break;
        }
    }
    tiles
}

#[cfg(test)]
mod tests {
    use super::tiles_along_segment;

    #[test]
    fn zero_length_segment_is_the_origin_tile() {
        assert_eq!(tiles_along_segment(4.0, 4.0, 4.0, 4.0), vec![(0, 0)]);
    }

    #[test]
    fn horizontal_segment_visits_each_crossed_tile() {
        let tiles = tiles_along_segment(4.0, 4.0, 20.0, 4.0);
        assert_eq!(tiles, vec![(0, 0), (1, 0), (2, 0)]);
    }

    #[test]
    fn diagonal_segment_includes_start_and_end() {
        let tiles = tiles_along_segment(4.0, 4.0, 20.0, 20.0);
        assert_eq!(tiles.first().copied(), Some((0, 0)));
        assert_eq!(tiles.last().copied(), Some((2, 2)));
        assert!(tiles.contains(&(1, 1)));
    }
}
