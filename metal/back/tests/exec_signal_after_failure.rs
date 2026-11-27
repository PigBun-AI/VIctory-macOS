//! A command that fails in a block must not take the block's fence signals with it: the title
//! waits for the fence, and a signal never given is a hang without a log (0% CPU).
#![cfg(target_vendor = "apple")]

mod exec_support;

use exec_support::*;
use victory_metal_proto::status::Status;

#[test]
fn a_fence_signalled_after_a_failing_command_is_still_given() {
    let fx = Fx::new();
    let q = fx.queue();
    let fence = fx.fence(0);
    // A dispatch with no pipeline set fails in the stream, in the middle of the block.
    let status = fx.submit(&[
        begin(1, q),
        dispatch(1, 1, 1),
        end(1),
        signal_cmd(q, fence, 5),
    ]);
    println!("block status {status:?}: {:?}", fx.executor().last_error());
    assert_ne!(status, Status::OK, "the block does fail");
    assert_eq!(
        fx.wait_for(fence, 5, 10_000),
        Status::OK,
        "the fence after the failure is signalled"
    );
}
