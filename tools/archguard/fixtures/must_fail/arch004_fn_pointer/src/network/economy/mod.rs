pub fn tick() {
    let f = crate::network::outbound::broadcast;
    f();
}
