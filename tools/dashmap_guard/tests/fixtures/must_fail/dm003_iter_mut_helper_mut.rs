use dashmap::DashMap;

struct World {
    tiles: DashMap<i32, i32>,
}

fn helper(world: &World) {
    world.tiles.insert(9, 9);
}

fn bug(world: &World) {
    for mut tile in world.tiles.iter_mut() {
        helper(world);
        let _ = tile.value_mut();
    }
}
