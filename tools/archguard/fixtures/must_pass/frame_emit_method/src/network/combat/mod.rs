use crate::network::outbound::FrameEmit;

pub fn tick(out: &dyn FrameEmit) {
    out.broadcast(vec![1, 2, 3]);
}
