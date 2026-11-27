//! `EXECUTE_INDIRECT` on a compute list: the signature of the command
//! against the root signature, as the plan the core
//! ([`crate::indirect`]) executes.

use victory_metal_proto::indirect::{ExecuteIndirect, IndirectArgKind};
use victory_metal_proto::status::Status;
use victory_metal_proto::types::RootParamKind;

use super::RootSig;
use super::tlab::SlotKind;
use crate::indirect::RootCopy;

/// What one `EXECUTE_INDIRECT` does, in the core's terms.
pub(super) struct Plan {
    pub roots: Vec<RootCopy>,
    /// Where the dispatch triple is in a record.
    pub dispatch_offset: u32,
    /// Bytes of a record.
    pub record_bytes: u32,
}

/// Checks the signature against the list's root signature `bound` and
/// works out where each argument is. Refuses what is not compute work
/// (`UNSUPPORTED`: the draw and mesh forms wait for the render encoder) and
/// what does not fit the root signature (`INVALID_ARG`).
pub(super) fn plan(c: &ExecuteIndirect<'_>, bound: Option<&RootSig>) -> Result<Plan, Status> {
    let mut roots = Vec::new();
    let mut at = 0u32;
    let mut dispatch_offset = None;
    for a in c.arguments() {
        let bytes = a.record_bytes() as u32;
        match a.kind {
            IndirectArgKind::DISPATCH => dispatch_offset = Some(at),
            IndirectArgKind::CONSTANT => {
                let sig = bound.ok_or(Status::INVALID_ARG)?;
                let slot = sig.layout.slot(a.index).ok_or(Status::INVALID_ARG)?;
                let SlotKind::Constants { count } = slot.kind else {
                    return Err(Status::INVALID_ARG);
                };
                if u64::from(a.dest_offset) + u64::from(a.count) > u64::from(count) {
                    return Err(Status::INVALID_ARG);
                }
                roots.push(RootCopy {
                    rec_word: at / 4,
                    tlab_word: slot.offset / 4 + a.dest_offset,
                    words: a.count,
                });
            }
            IndirectArgKind::CONSTANT_BUFFER_VIEW
            | IndirectArgKind::SHADER_RESOURCE_VIEW
            | IndirectArgKind::UNORDERED_ACCESS_VIEW => {
                let sig = bound.ok_or(Status::INVALID_ARG)?;
                let slot = sig.layout.slot(a.index).ok_or(Status::INVALID_ARG)?;
                let want = match a.kind {
                    IndirectArgKind::CONSTANT_BUFFER_VIEW => RootParamKind::CBV,
                    IndirectArgKind::SHADER_RESOURCE_VIEW => RootParamKind::SRV,
                    _ => RootParamKind::UAV,
                };
                if slot.kind != SlotKind::Descriptor(want) {
                    return Err(Status::INVALID_ARG);
                }
                roots.push(RootCopy {
                    rec_word: at / 4,
                    tlab_word: slot.offset / 4,
                    words: 2,
                });
            }
            // Draws, mesh dispatch, vertex and index buffer views, rays: the
            // numbers are reserved, the work is not done here.
            _ => return Err(Status::UNSUPPORTED),
        }
        at += bytes;
    }
    Ok(Plan {
        roots,
        dispatch_offset: dispatch_offset.ok_or(Status::INVALID_ARG)?,
        record_bytes: at,
    })
}

/// Whether the argument is one this back does for a compute list.
pub(super) fn is_compute_arg(kind: IndirectArgKind) -> bool {
    matches!(
        kind,
        IndirectArgKind::DISPATCH
            | IndirectArgKind::CONSTANT
            | IndirectArgKind::CONSTANT_BUFFER_VIEW
            | IndirectArgKind::SHADER_RESOURCE_VIEW
            | IndirectArgKind::UNORDERED_ACCESS_VIEW
    )
}
