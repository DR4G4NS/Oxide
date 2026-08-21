use dashmap::DashMap;

struct L3 {
    map: DashMap<i32, i32>,
}

struct L2 {
    x: L3,
    y: L3,
}

struct L1 {
    x: L2,
    y: L2,
}

fn distinct_paths(root: &L1) {
    let _ = root.x.x.map.get(&1);
    let _ = root.x.y.map.get(&2);
}
