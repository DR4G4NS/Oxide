use dashmap::DashMap;

struct Admin {
    map: DashMap<u32, u32>,
}

struct Console {
    admin: Admin,
}

impl Admin {
    fn set_value(&self) {
        self.map.insert(1, 2);
    }
}

impl Console {
    fn set_value(&self) {
        self.admin.set_value();
    }
}

fn bug(console: &Console) {
    let guard = console.admin.map.get(&1).unwrap();
    console.set_value();
    let _ = guard;
}
