use oxide::engine::world_stream;
fn main() -> std::io::Result<()> {
    let template = std::fs::read("src/dummy_world.dat")?;
    let map = world_stream::inspect_map(&template)?;
    let w = i32::from(map.width);
    let mut core_tiles = Vec::new();
    for y in 90..110i32 {
        for x in 30..50i32 {
            let i = (y * w + x) as usize;
            if map.blocks[i] == 341 {
                core_tiles.push((x, y));
            }
        }
    }
    println!("core tiles: {:?}", core_tiles);
    let core = map
        .buildings
        .iter()
        .find(|b| (339..=344).contains(&b.block))
        .unwrap();
    println!(
        "building pos: x={} y={}",
        core.position >> 16,
        core.position & 0xffff
    );
    Ok(())
}
