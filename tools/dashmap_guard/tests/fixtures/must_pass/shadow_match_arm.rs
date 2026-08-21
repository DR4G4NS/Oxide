use dashmap::DashMap;

struct World {
    tiles: DashMap<i32, i32>,
}

fn ok(world: &World) {
    let tiles = &world.tiles;
    match Some(1) {
        Some(tiles) => {
            let _ = tiles;
        }
        _ => {}
    }
    let guard = tiles.get(&1).unwrap();
    let _ = guard;
}
