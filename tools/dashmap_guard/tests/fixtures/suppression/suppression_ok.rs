// A narrow, justified suppression of a warning-only pattern.
fn callback_captured(world: &MyWorld) {
    // dashmap-guard: allow DM900 reason="dynamic callback proven not to access tiles"
    world.deferred_tick();
}
