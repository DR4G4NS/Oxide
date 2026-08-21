use dashmap::DashMap;

struct World {
    tiles: DashMap<i32, i32>,
}

fn bug(world: &World) {
    let refs: Vec<_> = world.tiles.iter().collect();
    world.tiles.insert(1, 2);
    let _ = refs;
}
