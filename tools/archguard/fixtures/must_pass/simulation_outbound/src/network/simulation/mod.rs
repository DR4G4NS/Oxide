use crate::network::outbound::broadcast;
use crate::network::world::PendingConnection;
use dashmap::DashMap;

pub fn spawn_world_simulation(connections: &DashMap<i32, PendingConnection>) {
    broadcast(connections, vec![]);
}
