use crate::network::listener::enqueue_outbound_routed;

pub fn tick() {
    enqueue_outbound_routed();
}
