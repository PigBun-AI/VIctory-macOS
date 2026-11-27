//! The render commands: layouts, limits and what `check` refuses.

mod common;

use common::*;
use victory_metal_proto::Command;
use victory_metal_proto::render::*;
use victory_metal_proto::stream::{Decoder, Encoder};
use victory_metal_proto::types::TexViewDim;
use victory_metal_proto::wire::Wire;

fn encode_one(c: &Command<'_>) -> Result<Vec<u8>, victory_metal_proto::Error> {
    let mut e = Encoder::new(victory_metal_proto::Caps::default());
    e.push(c)?;
    Ok(e.finish())
}

fn decodes(c: Command<'_>) -> bool {
    match encode_one(&c) {
        Ok(b) => Decoder::new(&b).unwrap().collect_all().is_ok(),
        Err(_) => false,
    }
}

#[test]
fn sizes_are_what_the_docs_say() {
    assert_eq!(<RenderAttachment as Wire>::SIZE, 36);
    assert_eq!(<DepthAttachment as Wire>::SIZE, 32);
    assert_eq!(<BeginRenderPass as Wire>::SIZE, 8 + 8 * 36 + 32 + 8);
    assert_eq!(<EndRenderPass as Wire>::SIZE, 16);
    assert_eq!(<SetViewports as Wire>::SIZE, 8 + 16 * 24);
    assert_eq!(<SetScissors as Wire>::SIZE, 8 + 16 * 16);
    assert_eq!(<SetVertexBuffers as Wire>::SIZE, 8 + 16 * 16);
    assert_eq!(<ClearAttachment as Wire>::SIZE, 40 + 8 * 16);
}

#[test]
fn the_samples_are_accepted() {
    assert!(decodes(Command::BeginRenderPass(sample_begin_pass())));
    assert!(decodes(Command::EndRenderPass(sample_end_pass())));
    assert!(decodes(Command::ClearAttachment(sample_clear_attachment())));
}

#[test]
fn a_pass_with_a_bad_attachment_is_refused() {
    let mut b = sample_begin_pass();
    b.rt_count = 9;
    assert!(!decodes(Command::BeginRenderPass(b)));
    let mut b = sample_begin_pass();
    b.rt[0].dimension = TexViewDim::NONE;
    assert!(!decodes(Command::BeginRenderPass(b)));
    let mut b = sample_begin_pass();
    b.rt[0].slice_count = 0;
    assert!(!decodes(Command::BeginRenderPass(b)));
    let mut b = sample_begin_pass();
    b.rt[0].load = LoadAction(9);
    assert!(!decodes(Command::BeginRenderPass(b)));
    let mut b = sample_begin_pass();
    b.rt[5].res = victory_metal_proto::ResId(1);
    assert!(!decodes(Command::BeginRenderPass(b)), "past the count");
    let mut b = sample_begin_pass();
    b.rt[0].clear[2] = f32::NAN;
    assert!(!decodes(Command::BeginRenderPass(b)));
}

#[test]
fn state_commands_check_their_limits() {
    let mut v = SetViewports::from_slice(&[Viewport::new(0.0, 0.0, 1.0, 1.0)]);
    v.count = 17;
    assert!(!decodes(Command::SetViewports(v)));
    let mut s = SetScissors::from_slice(&[]);
    s.count = 17;
    assert!(!decodes(Command::SetScissors(s)));
    let vb = SetVertexBuffers::from_slice(
        15,
        &[VertexBufferView {
            va: 0,
            size: 0,
            stride: 0,
        }; 2],
    );
    assert!(
        !decodes(Command::SetVertexBuffers(vb)),
        "slot 16 does not exist"
    );
    let t = SetPrimitiveTopology {
        topology: Topology(0),
        _pad: 0,
    };
    assert!(!decodes(Command::SetPrimitiveTopology(t)));
    let i = SetIndexBuffer {
        va: 0,
        size: 0,
        format: IndexFormat(7),
    };
    assert!(!decodes(Command::SetIndexBuffer(i)));
}

#[test]
fn a_clear_names_a_target_and_planes() {
    let mut c = sample_clear_attachment();
    c.flags = 0;
    assert!(
        !decodes(Command::ClearAttachment(c)),
        "depth target without planes"
    );
    let mut c = sample_clear_attachment();
    c.target = 9;
    assert!(!decodes(Command::ClearAttachment(c)));
    let mut c = sample_clear_attachment();
    c.rect_count = 9;
    assert!(!decodes(Command::ClearAttachment(c)));
    let mut c = sample_clear_attachment();
    c.target = 0;
    c.flags = 0;
    c.rect_count = 0;
    assert!(
        decodes(Command::ClearAttachment(c)),
        "a colour clear of everything"
    );
}

#[test]
fn slices_of_the_state_commands_hand_back_what_was_put_in() {
    let v = SetViewports::from_slice(&[Viewport::new(1.0, 2.0, 3.0, 4.0); 3]);
    assert_eq!(v.viewports().len(), 3);
    let big = SetVertexBuffers::from_slice(
        0,
        &[VertexBufferView {
            va: 1,
            size: 2,
            stride: 3,
        }; 20],
    );
    assert_eq!(big.views().len(), MAX_VERTEX_BUFFERS);
}

#[test]
fn a_pass_without_attachments_says_its_size_and_one_with_attachments_does_not() {
    // No attachment: the size is required.
    let mut b = <BeginRenderPass as Wire>::ZERO;
    assert!(!decodes(Command::BeginRenderPass(b)));
    b.width = 64;
    assert!(!decodes(Command::BeginRenderPass(b)));
    b.height = 32;
    assert!(decodes(Command::BeginRenderPass(b)));
    b.height = 16385;
    assert!(!decodes(Command::BeginRenderPass(b)));
    // Attachments: the size must stay 0.
    let mut b = sample_begin_pass();
    b.width = 8;
    b.height = 8;
    assert!(!decodes(Command::BeginRenderPass(b)));
}
