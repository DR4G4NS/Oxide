trait Ops {
    fn copy(&self, world: &World);
}

struct World {
    tiles: dashmap::DashMap<i32, i32>,
}

fn caller<T: Ops>(service: &T, world: &World) {
    let guard = world.tiles.get(&1).unwrap();
    service.copy(world);
    let _ = guard;
}
