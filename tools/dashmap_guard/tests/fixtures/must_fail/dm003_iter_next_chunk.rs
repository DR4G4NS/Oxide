use dashmap::DashMap;

struct World {
    tiles: DashMap<i32, i32>,
}

fn bug(world: &World) {
    let mut iter = world.tiles.iter();
    let _ = iter.next_chunk::<2>();
    world.tiles.insert(2, 3);
}
