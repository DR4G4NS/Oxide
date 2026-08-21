use dashmap::DashMap;
pub struct PendingConnection;
pub type Registry = DashMap<i32, PendingConnection>;
