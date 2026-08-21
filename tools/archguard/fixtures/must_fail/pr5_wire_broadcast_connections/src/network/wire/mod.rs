use crate::network::world::PendingConnection;
use dashmap::DashMap;

pub fn broadcast(connections: &DashMap<i32, PendingConnection>, frame: Vec<u8>) {
    for _ in connections.iter() {
        crate::network::listener::enqueue_outbound_routed();
    }
    let _ = frame;
}
