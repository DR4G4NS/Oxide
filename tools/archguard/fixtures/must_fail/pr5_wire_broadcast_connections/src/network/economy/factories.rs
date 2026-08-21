//! Exact PR #5 disguised reverse-dep: domain -> wire::broadcast -> listener.

use crate::network::wire::broadcast;
use crate::network::world::PendingConnection;
use dashmap::DashMap;

pub fn simulate_factories(connections: &DashMap<i32, PendingConnection>) {
    broadcast(connections, b"frame".to_vec());
}
