use dashmap::DashMap;

struct World {
    tiles: DashMap<i32, i32>,
}

fn helper(world: &World) {
    let _ = world.tiles.get(&123);
}

fn bug(world: &World) {
    for mut tile in world.tiles.iter_mut() {
        helper(world);
        let _ = tile.value_mut();
    }
}
