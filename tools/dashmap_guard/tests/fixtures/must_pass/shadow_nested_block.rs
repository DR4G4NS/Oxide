use dashmap::DashMap;

struct World {
    tiles: DashMap<i32, i32>,
}

fn ok(world: &World) {
    let tiles = &world.tiles;
    {
        let tiles = 99;
        let _ = tiles;
    }
    let guard = tiles.get(&1).unwrap();
    let _ = guard;
}
