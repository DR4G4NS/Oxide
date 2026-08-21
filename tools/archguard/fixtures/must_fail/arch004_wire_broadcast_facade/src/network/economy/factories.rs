use crate::network::wire::broadcast;
use crate::network::world::PendingConnection;
use dashmap::DashMap;

pub fn tick(connections: &DashMap<i32, PendingConnection>, frame: Vec<u8>) {
    broadcast(connections, frame);
}
