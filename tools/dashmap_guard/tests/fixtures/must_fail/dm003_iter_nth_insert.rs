use dashmap::DashMap;

struct World {
    tiles: DashMap<i32, i32>,
}

fn bug(world: &World) {
    let mut it = world.tiles.iter();
    let _ = it.nth(1);
    world.tiles.insert(1, 2);
}
