use dashmap::DashMap;

struct World {
    tiles: DashMap<i32, i32>,
}

fn ok(world: &World) {
    let tiles = &world.tiles;
    for tiles in [1, 2, 3] {
        let _ = tiles;
    }
    let guard = tiles.get(&1).unwrap();
    let _ = guard;
}
