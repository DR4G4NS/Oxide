// DashMap behind Arc must still be recognized (production uses Arc<DashMap>).
struct ArcHolder {
    arc_tiles: std::sync::Arc<dashmap::DashMap<i32, i32>>,
}
fn dm001_arc_field(h: &ArcHolder) {
    let g = h.arc_tiles.get(&1).unwrap();
    h.arc_tiles.insert(2, 3);
    let _ = g;
}
