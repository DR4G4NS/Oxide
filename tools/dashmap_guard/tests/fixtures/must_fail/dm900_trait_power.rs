trait Ops {
    fn power(&self, world: &MyWorld);
}

struct MyWorld {
    tiles: dashmap::DashMap<i32, i32>,
}

fn caller<T: Ops>(service: &T, world: &MyWorld) {
    let guard = world.tiles.get(&1).unwrap();
    service.power(world);
    let _ = guard;
}
