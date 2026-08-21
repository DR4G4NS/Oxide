use crate::network::world::PendingConnection;
use dashmap::DashMap;

pub fn broadcast(_connections: &DashMap<i32, PendingConnection>, _frame: Vec<u8>) {}
