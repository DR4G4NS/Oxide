use dashmap::DashMap;

struct World {
    tiles: DashMap<i32, i32>,
}

fn ok(world: &World) {
    {
        let guard = world.tiles.get(&1).unwrap();
        let _ = guard;
    }
    world.tiles.insert(2, 3);
}
