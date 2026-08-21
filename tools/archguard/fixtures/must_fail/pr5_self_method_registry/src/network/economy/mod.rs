use crate::network::world::PendingConnection;
use dashmap::DashMap;

pub struct NetCtx {
    pub connections: DashMap<i32, PendingConnection>,
}

impl NetCtx {
    pub fn tick(&self) {
        let _ = self;
    }
}
