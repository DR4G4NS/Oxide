use super::outbound::broadcast as send_all;

pub fn tick() {
    send_all();
}
