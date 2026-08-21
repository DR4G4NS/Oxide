use crate::network::helper::Hub;

pub fn tick(h: &Hub) {
    h.send_all();
}
