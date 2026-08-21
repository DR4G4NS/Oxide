trait DynOps {
    fn dynamic_operation(&self, world: &MyWorld);
}

struct MyWorld {
    tiles: dashmap::DashMap<i32, i32>,
}

fn helper2<T: DynOps>(service: &T, world: &MyWorld) {
    service.dynamic_operation(world);
}

fn helper1<T: DynOps>(service: &T, world: &MyWorld) {
    helper2(service, world);
}

fn caller<T: DynOps>(service: &T, world: &MyWorld) {
    let guard = world.tiles.get(&1).unwrap();
    helper1(service, world);
    let _ = guard;
}
