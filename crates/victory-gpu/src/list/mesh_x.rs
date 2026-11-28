//! Slot 98 is a producer when it selects the amplification stage; its pointer
//! is a destination, not a buffer of already generated dispatch arguments.
//! Preserve existing playback pending a producer and payload implementation.
#![cfg_attr(not(windows), allow(dead_code))]

pub(crate) struct Shape(bool);
impl Shape {
    pub(crate) fn runs_as_direct(&self) -> bool { self.0 }
}

pub(crate) fn note(_x: u32, flags: u64, indirect: u64, payload: u64) -> Shape {
    let direct = indirect == 0; // existing playback, not a claim of support for AS or payloads
    crate::mac::drop_stats::mesh_x(flags, indirect != 0, payload != 0);
    if !direct && crate::mac::drop_stats::enabled() {
        crate::mac::drop_stats::dropped(crate::mac::drop_stats::Op::Mesh, "mesh", "dispatch_mesh_x", "dispatch_mesh_x:producer_unsupported".into(), String::new());
    }
    Shape(direct)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_destination_is_not_executed_as_an_argument_buffer() {
        assert!(!note(4, 1, 256, 512).runs_as_direct());
        assert!(!note(4, 0, 256, 0).runs_as_direct());
        assert!(note(4, 0, 0, 0).runs_as_direct());
    }
}
