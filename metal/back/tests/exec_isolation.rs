//! A command of a block that the back refuses fails alone (white branch, the
//! "block stopped" finding): the work before and after it, in the same list
//! and in the lists after it, still runs. `VICTORY_METAL_STRICT_BLOCK=1`
//! (`ExecOptions::strict_block`) restores the old stop of the whole block.
#![cfg(target_vendor = "apple")]

mod exec_support;

use exec_support::*;
use victory_metal_back::exec::ExecOptions;
use victory_metal_proto::cmd::Command;
use victory_metal_proto::status::Status;
use victory_metal_proto::types::*;

struct World {
    fx: Fx,
    q: victory_metal_proto::ids::QueueId,
    f: victory_metal_proto::ids::FenceId,
    a: CreateBufferArgs,
    b: CreateBufferArgs,
    c: CreateBufferArgs,
}

fn world(strict: bool) -> World {
    let fx = Fx::with_options(ExecOptions {
        strict_block: strict,
        ..small_options()
    });
    let q = fx.queue();
    let f = fx.fence(0);
    let mk = |fx: &Fx| {
        let b = fx.buffer(BufferKind::SHARED, 256);
        cpu_write(b.cpu_addr, &[0; 64]);
        b
    };
    let (a, b, c) = (mk(&fx), mk(&fx), mk(&fx));
    World { fx, q, f, a, b, c }
}

impl World {
    fn settle(&self) {
        let v = self.fx.fence_value(self.f) + 1;
        assert_eq!(self.fx.submit(&[signal_cmd(self.q, self.f, v)]), Status::OK);
        assert_eq!(self.fx.wait(self.f, v), Status::OK);
    }

    fn is(&self, buf: &CreateBufferArgs, byte: u8) -> bool {
        cpu_read(buf.cpu_addr, 64)
            .iter()
            .all(|&w| w == u32::from(byte) * 0x0101_0101)
    }

    /// List 1: fill a, a copy that reaches past its buffers (refused), fill b;
    /// list 2: fill c.
    fn block(&self) -> Vec<Command<'static>> {
        vec![
            begin(1, self.q),
            fill(self.a.res, 0x1111_1111, 0, 256),
            copy(self.b.res, self.a.res, 4096),
            fill(self.b.res, 0x2222_2222, 0, 256),
            end(1),
            begin(2, self.q),
            fill(self.c.res, 0x3333_3333, 0, 256),
            end(2),
        ]
    }
}

#[test]
fn a_refused_command_fails_alone_and_the_rest_of_the_block_runs() {
    let w = world(false);
    // The block answers the refusal's status, and has run all the same.
    assert_eq!(w.fx.submit(&w.block()), Status::INVALID_ARG);
    w.settle();
    assert!(w.is(&w.a, 0x11), "work before the refused command");
    assert!(w.is(&w.b, 0x22), "work after it, in its list");
    assert!(w.is(&w.c, 0x33), "the list after it");
    assert_eq!(w.fx.executor().isolated_commands(), 1);
    let why = w.fx.executor().last_error().unwrap();
    assert!(
        why.contains("failed alone") && !why.contains("block stopped"),
        "{why}"
    );
}

#[test]
fn strict_block_restores_the_stop_of_the_whole_block() {
    let w = world(true);
    assert_ne!(w.fx.submit(&w.block()), Status::OK);
    w.settle();
    assert!(w.is(&w.a, 0), "the list is lost with the refused command");
    assert!(w.is(&w.b, 0));
    assert!(w.is(&w.c, 0), "the list after it never ran");
    assert_eq!(w.fx.executor().isolated_commands(), 0);
    assert!(
        w.fx.executor()
            .last_error()
            .unwrap()
            .contains("block stopped")
    );
}

/// A dispatch with no pipeline bound is refused with a reason that names it
/// (dropstats groups drops by the reason).
#[test]
fn a_dispatch_with_no_pipeline_says_why() {
    let w = world(false);
    let s = w.fx.submit(&[begin(1, w.q), dispatch(1, 1, 1), end(1)]);
    assert_eq!(s, Status::INVALID_ARG);
    let why = w.fx.executor().last_error().unwrap();
    assert!(why.contains("no compute pipeline bound"), "{why}");
}
