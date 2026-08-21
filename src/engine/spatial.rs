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
}
