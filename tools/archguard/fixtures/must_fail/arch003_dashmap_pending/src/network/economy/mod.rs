use crate::network::world::PendingConnection;
use dashmap::DashMap;

pub fn tick(connections: &DashMap<i32, PendingConnection>) {
    let _ = connections;
}
