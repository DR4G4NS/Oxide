#![allow(dead_code)]

use crate::engine::spatial::SpatialHashGrid;
use parking_lot::RwLock;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Default)]
pub struct Tile {
    pub x: i16,
    pub y: i16,
    pub floor_id: u16,
    pub overlay_id: u16,
    pub block_id: u16,
    pub team: u8,
}

impl Tile {
    pub fn empty(x: i16, y: i16) -> Self {
        Self {
            x,
            y,
            floor_id: 0,
            overlay_id: 0,
            block_id: 0,
            team: 0,
        }
    }
}

/// Cache-optimized tile grid chunking to eliminate per-tile RwLock overhead
pub const MAP_CHUNK_SIZE: usize = 32;

pub struct Chunk {
    pub tiles: [Tile; MAP_CHUNK_SIZE * MAP_CHUNK_SIZE],
}

pub struct World {
    pub width: usize,
    pub height: usize,
    pub chunks_x: usize,
    pub chunks_y: usize,
    pub chunk_grid: Vec<RwLock<Chunk>>,
    pub spatial_grid: Arc<SpatialHashGrid>,
}

impl World {
    pub fn new(width: usize, height: usize) -> Self {
        let chunks_x = width.div_ceil(MAP_CHUNK_SIZE);
        let chunks_y = height.div_ceil(MAP_CHUNK_SIZE);
        let total_chunks = chunks_x * chunks_y;

        let mut chunk_grid = Vec::with_capacity(total_chunks);
        for _ in 0..total_chunks {
            chunk_grid.push(RwLock::new(Chunk {
                tiles: [Tile::default(); MAP_CHUNK_SIZE * MAP_CHUNK_SIZE],
            }));
        }

        Self {
            width,
            height,
            chunks_x,
            chunks_y,
            chunk_grid,
            spatial_grid: Arc::new(SpatialHashGrid::new(width, height)),
        }
    }

    pub fn get_chunk(&self, cx: usize, cy: usize) -> Option<&RwLock<Chunk>> {
        if cx < self.chunks_x && cy < self.chunks_y {
            Some(&self.chunk_grid[cx + cy * self.chunks_x])
        } else {
            None
        }
    }
}
