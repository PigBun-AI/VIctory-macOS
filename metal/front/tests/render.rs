//! Render pass derivation: what the front puts in the command stream for
//! the render calls an application makes (no back involved: the recorded
//! items are read directly).

use std::sync::Arc;

use victory_metal_front::device::*;
use victory_metal_front::mock::MockLink;
use victory_metal_front::resource::{self, HeapProps, ResourceDesc};
use victory_metal_front::state;
use victory_metal_front::view::{self, TargetView};
use victory_metal_proto::render::*;
use victory_metal_proto::{Command, ResId};

fn setup() -> (Arc<MockLink>, Arc<Device>) {
    let link = Arc::new(MockLink::new());
    let sys = System::open(link.clone()).unwrap();
    (link, Device::new(sys))
}

fn tex(dev: &Arc<Device>, format: u32, flags: u32) -> (Resource, TargetView) {
    let desc = ResourceDesc {
        dimension: resource::DIM_TEXTURE2D,
        alignment: 0,
        width: 64,
        height: 32,
        depth_or_array: 1,
        mips: 1,
        format,
        sample_count: 1,
        sample_quality: 0,
        layout: resource::LAYOUT_UNKNOWN,
        flags,
    };
    let r = dev
        .create_committed(
            HeapProps::of_type(resource::HEAP_DEFAULT),
            &desc,
            if flags & resource::FLAG_ALLOW_DEPTH_STENCIL != 0 {
                state::DEPTH_WRITE
            } else {
                state::RENDER_TARGET
            },
        )
        .unwrap();
    let ds = flags & resource::FLAG_ALLOW_DEPTH_STENCIL != 0;
    let v = view::default_target(r.res, &desc, ds).unwrap();
    (r, v)
}

const RGBA8: u32 = 28;
const D32: u32 = 40;

fn rt() -> u32 {
    resource::FLAG_ALLOW_RENDER_TARGET
}

fn names(items: &[Item]) -> Vec<&'static str> {
    items
        .iter()
        .map(|i| match i {
            Item::Cmd(c) => match c {
                Command::BeginRenderPass(_) => "begin",
                Command::EndRenderPass(_) => "end",
                Command::Draw(_) => "draw",
                Command::DrawIndexed(_) => "draw_indexed",
                Command::DispatchMesh(_) => "mesh",
                Command::ClearAttachment(_) => "clear_attachment",
                Command::SetViewports(_) => "viewports",
                Command::SetScissors(_) => "scissors",
                Command::SetPipeline(_) => "pipeline",
                Command::Barrier(_) => "barrier",
                Command::CopyBufferRegion(_) => "copy",
                Command::Dispatch(_) => "dispatch",
                Command::SetPrimitiveTopology(_) => "topology",
                Command::SetIndexBuffer(_) => "index_buffer",
                Command::SetVertexBuffers(_) => "vertex_buffers",
                Command::SetBlendFactor(_) => "blend",
                Command::SetStencilRef(_) => "stencil",
                _ => "other",
            },
            Item::RootConstants { .. } => "root_constants",
        })
        .collect()
}

fn begin_of(items: &[Item], n: usize) -> BeginRenderPass {
    items
        .iter()
        .filter_map(|i| match i {
            Item::Cmd(Command::BeginRenderPass(b)) => Some(*b),
            _ => None,
        })
        .nth(n)
        .expect("a begin")
}

fn end_of(items: &[Item], n: usize) -> EndRenderPass {
    items
        .iter()
        .filter_map(|i| match i {
            Item::Cmd(Command::EndRenderPass(b)) => Some(*b),
            _ => None,
        })
        .nth(n)
        .expect("an end")
}

fn list(dev: &Arc<Device>) -> CommandList {
    CommandList::new(dev, 0)
}

const WHOLE: [Option<TargetView>; 8] = [None; 8];

fn rts(v: TargetView) -> [Option<TargetView>; 8] {
    let mut a = WHOLE;
    a[0] = Some(v);
    a
}

#[test]
fn a_pass_begins_at_the_first_draw_and_ends_at_close() {
    let (_l, dev) = setup();
    let (_r, v) = tex(&dev, RGBA8, rt());
    let mut l = list(&dev);
    l.set_render_targets(rts(v), None);
    l.set_topology(4);
    assert_eq!(names(&l.close().unwrap_or_default()), ["topology"]);
    let mut l = list(&dev);
    l.set_render_targets(rts(v), None);
    l.set_viewports(&[Viewport::new(0.0, 0.0, 64.0, 32.0)]);
    l.draw(3, 1, 0, 0);
    l.draw(3, 1, 0, 0);
    let items = l.close().unwrap();
    assert_eq!(names(&items), ["viewports", "begin", "draw", "draw", "end"]);
    let b = begin_of(&items, 0);
    assert_eq!(b.rt_count, 1);
    assert_eq!(b.rt[0].res, v.res);
    assert_eq!(b.rt[0].load, LoadAction::LOAD);
    assert!(b.ds.res.is_null());
    assert_eq!(end_of(&items, 0).rt_store[0], StoreAction::STORE);
    // A second close adds nothing.
    assert_eq!(l.close().unwrap().len(), 5);
}

#[test]
fn a_full_clear_before_the_first_draw_is_the_load_action() {
    let (_l, dev) = setup();
    let (_r, v) = tex(&dev, RGBA8, rt());
    let (_d, dv) = tex(&dev, D32, resource::FLAG_ALLOW_DEPTH_STENCIL);
    let mut l = list(&dev);
    l.set_render_targets(rts(v), Some(dv));
    l.clear_render_target(Some(v), [0.1, 0.2, 0.3, 0.4], &[]);
    l.clear_depth_stencil(Some(dv), 1, 0.5, 0, &[]);
    l.draw(3, 1, 0, 0);
    let items = l.close().unwrap();
    assert_eq!(names(&items), ["begin", "draw", "end"]);
    let b = begin_of(&items, 0);
    assert_eq!(b.rt[0].load, LoadAction::CLEAR);
    assert_eq!(b.rt[0].clear, [0.1, 0.2, 0.3, 0.4]);
    assert_eq!(b.ds.depth_load, LoadAction::CLEAR);
    assert_eq!(b.ds.stencil_load, LoadAction::LOAD);
    assert_eq!(b.ds.clear_depth, 0.5);
}

#[test]
fn a_clear_with_rectangles_or_after_drawing_is_a_clear_inside_the_pass() {
    let (_l, dev) = setup();
    let (_r, v) = tex(&dev, RGBA8, rt());
    let mut l = list(&dev);
    l.set_render_targets(rts(v), None);
    l.draw(3, 1, 0, 0);
    l.clear_render_target(Some(v), [1.0, 0.0, 0.0, 1.0], &[]);
    let many: Vec<[i32; 4]> = (0..10).map(|i| [i, 0, i + 1, 4]).collect();
    l.clear_render_target(Some(v), [0.0, 1.0, 0.0, 1.0], &many);
    let items = l.close().unwrap();
    // Ten rectangles are two commands (8 + 2).
    assert_eq!(
        names(&items),
        [
            "begin",
            "draw",
            "clear_attachment",
            "clear_attachment",
            "clear_attachment",
            "end"
        ]
    );
    // A rectangle clear with no draw before it begins the pass with LOAD.
    let mut l = list(&dev);
    l.set_render_targets(rts(v), None);
    l.clear_render_target(Some(v), [0.0; 4], &[[0, 0, 8, 8]]);
    let items = l.close().unwrap();
    assert_eq!(names(&items), ["begin", "clear_attachment", "end"]);
    assert_eq!(begin_of(&items, 0).rt[0].load, LoadAction::LOAD);
}

#[test]
fn clears_with_nothing_drawn_make_clear_only_passes() {
    let (_l, dev) = setup();
    let (_r, v) = tex(&dev, RGBA8, rt());
    let (_o, other) = tex(&dev, RGBA8, rt());
    let mut l = list(&dev);
    // Bound and cleared, never drawn: a pass of loads and the clear.
    l.set_render_targets(rts(v), None);
    l.clear_render_target(Some(v), [0.5; 4], &[]);
    // A view that is not bound: a pass of its own.
    l.clear_render_target(Some(other), [0.25; 4], &[]);
    let items = l.close().unwrap();
    assert_eq!(names(&items), ["begin", "end", "begin", "end"]);
    assert_eq!(begin_of(&items, 0).rt[0].res, v.res);
    assert_eq!(begin_of(&items, 0).rt[0].load, LoadAction::CLEAR);
    assert_eq!(begin_of(&items, 1).rt[0].res, other.res);
    assert_eq!(begin_of(&items, 1).rt[0].clear, [0.25; 4]);
}

#[test]
fn changing_targets_or_a_non_render_command_ends_the_pass_state_commands_do_not() {
    let (_l, dev) = setup();
    let (_a, a) = tex(&dev, RGBA8, rt());
    let (_b, b) = tex(&dev, RGBA8, rt());
    let buf = dev
        .create_committed_buffer(
            resource::HEAP_DEFAULT,
            &ResourceDesc::buffer(256, resource::FLAG_ALLOW_UNORDERED_ACCESS),
            0,
        )
        .unwrap();
    let buf2 = dev
        .create_committed_buffer(
            resource::HEAP_DEFAULT,
            &ResourceDesc::buffer(256, resource::FLAG_ALLOW_UNORDERED_ACCESS),
            0,
        )
        .unwrap();
    let mut l = list(&dev);
    l.set_render_targets(rts(a), None);
    l.draw(3, 1, 0, 0);
    // The same set again, state, a root constant: still the same pass.
    l.set_render_targets(rts(a), None);
    l.set_blend_factor([1.0; 4]);
    l.set_stencil_ref(3);
    l.set_root_constants(victory_metal_proto::types::BindPoint::GRAPHICS, 0, 0, &[1]);
    l.draw(3, 1, 0, 0);
    // A copy ends it; the next draw begins a new pass that loads.
    l.copy_buffer_region(&buf, 0, &buf2, 0, 64);
    l.draw(3, 1, 0, 0);
    // Another target set: a new pass.
    l.set_render_targets(rts(b), None);
    l.draw(3, 1, 0, 0);
    let items = l.close().unwrap();
    assert_eq!(
        names(&items),
        [
            "begin",
            "draw",
            "blend",
            "stencil",
            "root_constants",
            "draw",
            "end",
            "copy",
            "begin",
            "draw",
            "end",
            "begin",
            "draw",
            "end"
        ]
    );
    assert_eq!(begin_of(&items, 1).rt[0].load, LoadAction::LOAD);
    assert_eq!(begin_of(&items, 2).rt[0].res, b.res);
}

#[test]
fn a_transition_of_an_attachment_ends_the_pass_other_transitions_do_not() {
    let (_l, dev) = setup();
    let (a_res, a) = tex(&dev, RGBA8, rt());
    let (b_res, _b) = tex(&dev, RGBA8, rt());
    let mut l = list(&dev);
    l.set_render_targets(rts(a), None);
    l.draw(3, 1, 0, 0);
    use ResourceBarrier::Transition;
    l.resource_barrier(&[Transition {
        res: b_res.res,
        subresource: 0xffff_ffff,
        before: state::RENDER_TARGET,
        after: state::PIXEL_SHADER_RESOURCE,
        flags: 0,
    }]);
    l.draw(3, 1, 0, 0);
    l.resource_barrier(&[Transition {
        res: a_res.res,
        subresource: 0xffff_ffff,
        before: state::RENDER_TARGET,
        after: state::PIXEL_SHADER_RESOURCE,
        flags: 0,
    }]);
    let items = l.close().unwrap();
    let n = names(&items);
    assert_eq!(n.iter().filter(|x| **x == "begin").count(), 1);
    // The pass ends before the barrier of the attachment, not before the other.
    let end = n.iter().position(|x| *x == "end").unwrap();
    let barriers: Vec<usize> = n
        .iter()
        .enumerate()
        .filter(|(_, x)| **x == "barrier")
        .map(|(i, _)| i)
        .collect();
    assert_eq!(barriers.len(), 2);
    assert!(barriers[0] < end && end < barriers[1], "{n:?}");
}

#[test]
fn a_discard_after_the_pass_turns_the_store_into_dont_care_and_the_next_load_too() {
    let (_l, dev) = setup();
    let (a_res, a) = tex(&dev, RGBA8, rt());
    let (d_res, d) = tex(&dev, D32, resource::FLAG_ALLOW_DEPTH_STENCIL);
    let mut l = list(&dev);
    l.set_render_targets(rts(a), Some(d));
    l.draw(3, 1, 0, 0);
    // Ends the pass (barrier of the depth buffer), then the discards.
    l.resource_barrier(&[ResourceBarrier::Transition {
        res: d_res.res,
        subresource: 0xffff_ffff,
        before: state::DEPTH_WRITE,
        after: state::DEPTH_READ,
        flags: 0,
    }]);
    l.discard_resource(d_res.res);
    l.discard_resource(a_res.res);
    // The next pass over them does not load what was discarded.
    l.draw(3, 1, 0, 0);
    let items = l.close().unwrap();
    let e = end_of(&items, 0);
    assert_eq!(e.rt_store[0], StoreAction::DONT_CARE);
    assert_eq!(e.depth_store, StoreAction::DONT_CARE);
    let b = begin_of(&items, 1);
    assert_eq!(b.rt[0].load, LoadAction::DONT_CARE);
    assert_eq!(b.ds.depth_load, LoadAction::DONT_CARE);
    // A discard after other commands does not reach back.
    let mut l = list(&dev);
    l.set_render_targets(rts(a), None);
    l.draw(3, 1, 0, 0);
    l.dispatch(1, 1, 1);
    l.discard_resource(a_res.res);
    let items = l.close().unwrap();
    assert_eq!(end_of(&items, 0).rt_store[0], StoreAction::STORE);
}

#[test]
fn draws_without_targets_and_bad_arguments_fail_the_list() {
    let (_l, dev) = setup();
    let mut l = list(&dev);
    l.draw(3, 1, 0, 0);
    assert!(l.close().is_err());
    let mut l = list(&dev);
    l.set_topology(12);
    assert!(l.close().is_err());
    let mut l = list(&dev);
    l.set_index_buffer(0x1000, 64, 99);
    assert!(l.close().is_err());
    let mut l = list(&dev);
    l.set_vertex_buffers(15, &[(0x1000, 64, 12), (0x2000, 64, 12)]);
    assert!(l.close().is_err());
}

#[test]
fn the_state_commands_map_to_proto() {
    let (_l, dev) = setup();
    let (_r, v) = tex(&dev, RGBA8, rt());
    let mut l = list(&dev);
    l.set_render_targets(rts(v), None);
    l.set_topology(5);
    l.set_index_buffer(0x4000, 128, 57);
    l.set_vertex_buffers(1, &[(0x8000, 96, 24)]);
    l.set_scissors(&[[1, 2, 3, 4]]);
    l.draw_indexed(6, 2, 1, -3, 7);
    l.dispatch_mesh(4, 5, 6);
    let items = l.close().unwrap();
    let cmds: Vec<&Command> = items
        .iter()
        .filter_map(|i| match i {
            Item::Cmd(c) => Some(c),
            _ => None,
        })
        .collect();
    assert!(
        matches!(cmds[0], Command::SetPrimitiveTopology(t) if t.topology == Topology::TRIANGLE_STRIP)
    );
    assert!(
        matches!(cmds[1], Command::SetIndexBuffer(i) if i.format == IndexFormat::U16 && i.va == 0x4000)
    );
    assert!(
        matches!(cmds[2], Command::SetVertexBuffers(s) if s.start_slot == 1 && s.views()[0].stride == 24)
    );
    assert!(matches!(cmds[3], Command::SetScissors(s) if s.rects()[0] == Rect::new(1, 2, 3, 4)));
    assert!(
        matches!(cmds[5], Command::DrawIndexed(d) if d.base_vertex == -3 && d.start_instance == 7)
    );
    assert!(matches!(cmds[6], Command::DispatchMesh(m) if (m.x, m.y, m.z) == (4, 5, 6)));
    let _ = ResId::NULL;
}
