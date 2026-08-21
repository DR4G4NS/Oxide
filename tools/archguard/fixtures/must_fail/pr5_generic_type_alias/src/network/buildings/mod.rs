use crate::network::world::PendingConnection;
use dashmap::DashMap;

type Registry<K> = DashMap<K, PendingConnection>;

pub fn tick(c: &Registry<i32>) {
    let _ = c;
}
