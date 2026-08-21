use crate::network::outbound::FrameEmit;

pub fn tick(out: &dyn FrameEmit) {
    FrameEmit::broadcast(out, vec![1]);
}
