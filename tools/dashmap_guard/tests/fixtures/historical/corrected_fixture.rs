// Corrected form: snapshot the field, then call setblock. MUST pass.
fn getblock_and_setblock_corrected(world: &mut MyWorld) {
    let a = 1;
    let block = world.tiles.get(&a).map(|t| t.block).unwrap();
    assert_eq!(block, 1);
    setblock(world, 2, 2);
}
