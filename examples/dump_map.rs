use oxide::engine::world_stream;
use std::io::Write;

fn main() -> std::io::Result<()> {
    let template = std::fs::read("src/dummy_world.dat")?;
    let map = world_stream::inspect_map(&template)?;
    let mut out = std::fs::File::create("/tmp/dumpmap/blocks.tsv")?;
    for (i, block) in map.blocks.iter().enumerate() {
        let x = i % map.width as usize;
        let y = i / map.width as usize;
        if *block != 0 {
            writeln!(out, "{}\t{}\t{}", x, y, block)?;
        }
    }
    Ok(())
}
