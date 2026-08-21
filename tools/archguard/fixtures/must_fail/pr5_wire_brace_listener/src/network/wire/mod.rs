use crate::network::{listener, runtime};

pub fn helper() {
    listener::enqueue_outbound_routed();
    runtime::save_slot_path();
}
