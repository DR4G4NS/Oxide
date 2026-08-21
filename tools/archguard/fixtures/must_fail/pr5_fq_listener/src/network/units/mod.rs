pub fn tick() {
    crate::network::listener::enqueue_outbound_routed();
}
