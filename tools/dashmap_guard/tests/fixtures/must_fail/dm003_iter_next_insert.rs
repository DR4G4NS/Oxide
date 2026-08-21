use dashmap::DashMap;

struct World {
    tiles: DashMap<i32, i32>,
}

fn bug(world: &World) {
    let mut it = world.tiles.iter();
    let _ = it.next();
    world.tiles.insert(1, 2);
}
