pub struct Hub;
impl Hub {
    pub fn send_all(&self) {
        crate::network::outbound::broadcast();
    }
}
