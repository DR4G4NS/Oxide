use dashmap::DashMap;

type TileMap = DashMap<i32, i32>;

struct World {
    tiles: TileMap,
    other: TileMap,
}

fn ok(world: &World) {
    let guard = world.tiles.get(&1).unwrap();
    world.other.insert(2, 3);
    let _ = guard;
}
