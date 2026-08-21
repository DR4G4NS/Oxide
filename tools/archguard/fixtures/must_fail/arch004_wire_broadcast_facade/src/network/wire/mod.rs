use crate::network::world::PendingConnection;
use dashmap::DashMap;

pub fn broadcast(connections: &DashMap<i32, PendingConnection>, frame: Vec<u8>) {
    crate::network::listener::enqueue_outbound_routed();
    let _ = (connections, frame);
}
