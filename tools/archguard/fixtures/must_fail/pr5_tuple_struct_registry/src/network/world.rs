use dashmap::DashMap;
pub struct PendingConnection;
pub struct NetCtx(pub DashMap<i32, PendingConnection>);
