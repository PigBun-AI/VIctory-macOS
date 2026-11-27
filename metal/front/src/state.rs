//! Resource states: the D3D12 state bits, the state → stage-mask table and
//! the tracker that turns `ResourceBarrier` calls into stage barriers.
//!
//! The stage masks are the cross-layer convention (architecture 2.6 item 4):
//! `u16`, bit0 Vertex, bit1 Fragment, bit2 Tile, bit3 Object, bit4 Mesh,
//! bit5 Dispatch, bit6 Blit, bit7 ResourceState; `ALL = 0x00FF`. Visibility
//! is `u8`: bit0 Device, bit1 ResourceAlias. D3D12 state bits never leave the
//! front; the back sees only these masks.

use std::collections::HashMap;

pub mod stage {
    pub const VERTEX: u16 = 1 << 0;
    pub const FRAGMENT: u16 = 1 << 1;
    pub const TILE: u16 = 1 << 2;
    pub const OBJECT: u16 = 1 << 3;
    pub const MESH: u16 = 1 << 4;
    pub const DISPATCH: u16 = 1 << 5;
    pub const BLIT: u16 = 1 << 6;
    pub const RESOURCE_STATE: u16 = 1 << 7;
    pub const ALL: u16 = 0x00FF;
    /// Every programmable stage a UAV or a constant buffer can be touched in.
    pub const SHADERS: u16 = VERTEX | FRAGMENT | OBJECT | MESH | DISPATCH;
}

pub mod visibility {
    pub const DEVICE: u8 = 1 << 0;
    pub const RESOURCE_ALIAS: u8 = 1 << 1;
}

// D3D12_RESOURCE_STATES
// The numbers are proto's (one source of truth: the compiler checks).
const _: () = {
    use victory_metal_proto::types::{stage as p, visibility as v};
    assert!(stage::VERTEX == p::VERTEX && stage::FRAGMENT == p::FRAGMENT && stage::TILE == p::TILE);
    assert!(stage::OBJECT == p::OBJECT && stage::MESH == p::MESH && stage::DISPATCH == p::DISPATCH);
    assert!(
        stage::BLIT == p::BLIT
            && stage::RESOURCE_STATE == p::RESOURCE_STATE
            && stage::ALL == p::ALL
    );
    assert!(visibility::DEVICE == v::DEVICE && visibility::RESOURCE_ALIAS == v::RESOURCE_ALIAS);
};

pub const COMMON: u32 = 0;
pub const PRESENT: u32 = 0;
pub const VERTEX_AND_CONSTANT_BUFFER: u32 = 0x1;
pub const INDEX_BUFFER: u32 = 0x2;
pub const RENDER_TARGET: u32 = 0x4;
pub const UNORDERED_ACCESS: u32 = 0x8;
pub const DEPTH_WRITE: u32 = 0x10;
pub const DEPTH_READ: u32 = 0x20;
pub const NON_PIXEL_SHADER_RESOURCE: u32 = 0x40;
pub const PIXEL_SHADER_RESOURCE: u32 = 0x80;
pub const STREAM_OUT: u32 = 0x100;
pub const INDIRECT_ARGUMENT: u32 = 0x200;
pub const COPY_DEST: u32 = 0x400;
pub const COPY_SOURCE: u32 = 0x800;
pub const RESOLVE_DEST: u32 = 0x1000;
pub const RESOLVE_SOURCE: u32 = 0x2000;
pub const RAYTRACING_ACCELERATION_STRUCTURE: u32 = 0x40_0000;
pub const SHADING_RATE_SOURCE: u32 = 0x100_0000;
pub const GENERIC_READ: u32 = 0xAC3;

/// The stages that touch a resource while it is in `state`. Combined read
/// states OR together; `COMMON` / `PRESENT` have none.
pub fn stages_of(state: u32) -> u16 {
    use stage::*;
    let table: [(u32, u16); 16] = [
        (VERTEX_AND_CONSTANT_BUFFER, SHADERS),
        (INDEX_BUFFER, VERTEX),
        (RENDER_TARGET, FRAGMENT),
        (UNORDERED_ACCESS, SHADERS),
        (DEPTH_WRITE, FRAGMENT),
        (DEPTH_READ, FRAGMENT),
        (NON_PIXEL_SHADER_RESOURCE, VERTEX | OBJECT | MESH | DISPATCH),
        (PIXEL_SHADER_RESOURCE, FRAGMENT),
        (STREAM_OUT, VERTEX),
        (INDIRECT_ARGUMENT, DISPATCH | VERTEX | OBJECT | MESH),
        (COPY_DEST, BLIT),
        (COPY_SOURCE, BLIT),
        (RESOLVE_DEST, FRAGMENT),
        (RESOLVE_SOURCE, FRAGMENT),
        (RAYTRACING_ACCELERATION_STRUCTURE, DISPATCH | BLIT),
        (SHADING_RATE_SOURCE, FRAGMENT),
    ];
    table
        .iter()
        .filter(|(bit, _)| state & bit != 0)
        .fold(0, |m, (_, s)| m | s)
}

/// States that write the resource.
pub fn is_write_state(state: u32) -> bool {
    state & (RENDER_TARGET | UNORDERED_ACCESS | DEPTH_WRITE | STREAM_OUT | COPY_DEST | RESOLVE_DEST)
        != 0
}

/// What the back is told: wait for `before` stages, then let `after` stages
/// go on, with this visibility. Whole-resource: the back's barrier is
/// stage→stage and does not name a subresource.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StageBarrier {
    pub before: u16,
    pub after: u16,
    pub visibility: u8,
}

/// A transition from `before` to `after`. `None` when no barrier is needed:
/// same state, either side COMMON/PRESENT (decay and promotion are implicit
/// in D3D12), or no stages on either side.
pub fn transition_barrier(before: u32, after: u32) -> Option<StageBarrier> {
    if before == after || before == COMMON || after == COMMON {
        return None;
    }
    let (b, a) = (stages_of(before), stages_of(after));
    if b == 0 || a == 0 {
        return None;
    }
    // Two read states in a row need no memory barrier.
    if !is_write_state(before) && !is_write_state(after) {
        return None;
    }
    Some(StageBarrier {
        before: b,
        after: a,
        visibility: visibility::DEVICE,
    })
}

/// A UAV barrier: shader writes before, shader accesses after.
pub fn uav_barrier() -> StageBarrier {
    StageBarrier {
        before: stage::SHADERS,
        after: stage::SHADERS,
        visibility: visibility::DEVICE,
    }
}

/// An aliasing barrier: everything, including aliased pages.
pub fn aliasing_barrier() -> StageBarrier {
    StageBarrier {
        before: stage::ALL,
        after: stage::ALL,
        visibility: visibility::DEVICE | visibility::RESOURCE_ALIAS,
    }
}

/// `D3D12_RESOURCE_BARRIER_FLAG`.
pub const BARRIER_FLAG_BEGIN_ONLY: u32 = 1;
pub const BARRIER_FLAG_END_ONLY: u32 = 2;
pub const ALL_SUBRESOURCES: u32 = 0xffff_ffff;

/// Tracks the state of every resource's subresources as the front has seen
/// it, command list after command list. `ResourceBarrier` calls go through
/// [`StateTracker::transition`]; a split barrier is sent at its END only
/// (proto.md section 4: "send the barrier at the end point only"; the state
/// changes there, as in D3D12).
#[derive(Default)]
pub struct StateTracker {
    resources: HashMap<u32, Vec<u32>>,
    /// Transitions whose StateBefore differed from the tracked state.
    pub mismatches: u32,
}

#[derive(Debug, PartialEq, Eq)]
pub enum TransitionError {
    UnknownResource,
    SubresourceOutOfRange,
}

impl StateTracker {
    pub fn add(&mut self, resource: u32, subresources: u32, initial: u32) {
        self.resources
            .insert(resource, vec![initial; subresources.max(1) as usize]);
    }

    pub fn remove(&mut self, resource: u32) {
        self.resources.remove(&resource);
    }

    pub fn state(&self, resource: u32, subresource: u32) -> Option<u32> {
        self.resources
            .get(&resource)?
            .get(subresource as usize)
            .copied()
    }

    /// Apply a transition and return the barrier to record, if any. `flags`
    /// are the D3D12 barrier flags: BEGIN_ONLY records nothing and leaves the
    /// state (the barrier is emitted at END), END_ONLY and 0 emit and update.
    pub fn transition(
        &mut self,
        resource: u32,
        subresource: u32,
        before: u32,
        after: u32,
        flags: u32,
    ) -> Result<Option<StageBarrier>, TransitionError> {
        let subs = self
            .resources
            .get_mut(&resource)
            .ok_or(TransitionError::UnknownResource)?;
        if flags & BARRIER_FLAG_BEGIN_ONLY != 0 {
            return Ok(None);
        }
        let range = if subresource == ALL_SUBRESOURCES {
            0..subs.len()
        } else if (subresource as usize) < subs.len() {
            subresource as usize..subresource as usize + 1
        } else {
            return Err(TransitionError::SubresourceOutOfRange);
        };
        // One barrier for the whole range: the union of what each
        // subresource moves from.
        let mut merged: Option<StageBarrier> = None;
        for s in &mut subs[range] {
            if *s != before {
                self.mismatches += 1;
            }
            // The application's StateBefore is the rule (D3D12 requires it
            // to equal the tracked state).
            if let Some(b) = transition_barrier(before, after) {
                merged = Some(match merged {
                    Some(m) => StageBarrier {
                        before: m.before | b.before,
                        after: m.after | b.after,
                        visibility: m.visibility,
                    },
                    None => b,
                });
            }
            *s = after;
        }
        Ok(merged)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use stage::*;

    #[test]
    fn copy_dest_to_srv_waits_for_the_blit() {
        // The case the architecture names: COPY_DEST -> SRV.
        let b = transition_barrier(COPY_DEST, NON_PIXEL_SHADER_RESOURCE | PIXEL_SHADER_RESOURCE)
            .unwrap();
        assert_eq!(b.before, BLIT);
        assert_eq!(b.after, VERTEX | OBJECT | MESH | DISPATCH | FRAGMENT);
        assert_eq!(b.visibility, visibility::DEVICE);
        let b = transition_barrier(COPY_DEST, NON_PIXEL_SHADER_RESOURCE).unwrap();
        assert_eq!(b.after & FRAGMENT, 0);
    }

    #[test]
    fn common_and_present_emit_nothing() {
        assert_eq!(transition_barrier(COMMON, COPY_DEST), None);
        assert_eq!(transition_barrier(COPY_SOURCE, PRESENT), None);
        assert_eq!(transition_barrier(COPY_DEST, COPY_DEST), None);
        // Read -> read needs no memory barrier.
        assert_eq!(
            transition_barrier(COPY_SOURCE, NON_PIXEL_SHADER_RESOURCE),
            None
        );
        assert_eq!(transition_barrier(GENERIC_READ, COPY_SOURCE), None);
    }

    #[test]
    fn write_to_read_and_back() {
        let b = transition_barrier(UNORDERED_ACCESS, COPY_SOURCE).unwrap();
        assert_eq!((b.before, b.after), (SHADERS, BLIT));
        let b = transition_barrier(COPY_SOURCE, UNORDERED_ACCESS).unwrap();
        assert_eq!((b.before, b.after), (BLIT, SHADERS));
        let b = transition_barrier(RENDER_TARGET, PIXEL_SHADER_RESOURCE).unwrap();
        assert_eq!((b.before, b.after), (FRAGMENT, FRAGMENT));
    }

    #[test]
    fn masks_stay_inside_all() {
        for bit in 0..31 {
            assert_eq!(stages_of(1 << bit) & !ALL, 0);
        }
        assert_eq!(stages_of(GENERIC_READ) & BLIT, BLIT);
        assert_eq!(uav_barrier().before, SHADERS);
        assert_eq!(aliasing_barrier().visibility, 3);
    }

    #[test]
    fn tracker_follows_transitions_and_split_barriers() {
        let mut t = StateTracker::default();
        t.add(7, 3, COMMON);
        // All subresources at once.
        let b = t
            .transition(7, ALL_SUBRESOURCES, COMMON, COPY_DEST, 0)
            .unwrap();
        assert_eq!(b, None);
        assert_eq!(t.state(7, 2), Some(COPY_DEST));
        let b = t
            .transition(7, 1, COPY_DEST, NON_PIXEL_SHADER_RESOURCE, 0)
            .unwrap()
            .unwrap();
        assert_eq!(b.before, BLIT);
        assert_eq!(t.state(7, 1), Some(NON_PIXEL_SHADER_RESOURCE));
        assert_eq!(t.state(7, 0), Some(COPY_DEST));
        // Split barrier: nothing at BEGIN, emitted at END.
        let begin = t
            .transition(7, 0, COPY_DEST, UNORDERED_ACCESS, BARRIER_FLAG_BEGIN_ONLY)
            .unwrap();
        assert_eq!(begin, None);
        assert_eq!(t.state(7, 0), Some(COPY_DEST));
        let end = t
            .transition(7, 0, COPY_DEST, UNORDERED_ACCESS, BARRIER_FLAG_END_ONLY)
            .unwrap();
        assert!(end.is_some());
        assert_eq!(t.state(7, 0), Some(UNORDERED_ACCESS));
        assert_eq!(
            t.transition(8, 0, 0, 1, 0),
            Err(TransitionError::UnknownResource)
        );
        assert_eq!(
            t.transition(7, 3, 0, 1, 0),
            Err(TransitionError::SubresourceOutOfRange)
        );
        // A wrong StateBefore is counted, not fatal.
        t.transition(7, 2, UNORDERED_ACCESS, COPY_SOURCE, 0)
            .unwrap();
        assert_eq!(t.mismatches, 1);
    }
}
