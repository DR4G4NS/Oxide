use crate::network::world::PendingConnection;
use dashmap::DashMap;

pub struct NetCtx {
    pub connections: DashMap<i32, PendingConnection>,
}

pub fn tick(ctx: &NetCtx) {
    let _ = ctx;
}
