use crate::network::outbound::broadcast as send_all;

pub fn tick() {
    send_all();
}
