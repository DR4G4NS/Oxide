// Shared phantom types so fixtures stay syntactically and semantically valid
// without pulling in the real crate. The analyzer only needs to PARSE these.
#[derive(Clone, Copy, Default)]
pub struct DynamicTile {
    pub block: i16,
    pub team: u8,
    pub id: i32,
}

#[derive(Clone, Copy, Default)]
pub struct EnemyUnit {
    pub id: i32,
}

pub struct MyWorld {
    pub tiles: dashmap::DashMap<i32, DynamicTile>,
    pub enemies: dashmap::DashMap<i32, EnemyUnit>,
    pub connections: dashmap::DashMap<i32, i32>,
}
