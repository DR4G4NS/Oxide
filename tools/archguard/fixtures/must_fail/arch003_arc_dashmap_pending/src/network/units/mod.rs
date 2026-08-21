use crate::network::world::PendingConnection;
use dashmap::DashMap;
use std::sync::Arc;

pub fn tick(connections: Arc<DashMap<i32, PendingConnection>>) {
    let _ = connections;
}
