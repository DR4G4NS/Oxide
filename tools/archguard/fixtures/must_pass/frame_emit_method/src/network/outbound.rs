pub trait FrameEmit {
    fn broadcast(&self, frame: Vec<u8>);
}
