//! `EXECUTE_INDIRECT` of a compute, draw or mesh list: the signature of the command
//! against the root signature, as the plan the core
//! ([`crate::indirect`]) executes.

use victory_metal_proto::indirect::{ExecuteIndirect, IndirectArgKind};
use victory_metal_proto::status::Status;
use victory_metal_proto::types::RootParamKind;

use super::RootSig;
use super::tlab::SlotKind;
use crate::indirect::RootCopy;

/// The action a record ends with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Action {
    Dispatch,
    Draw,
    DrawIndexed,
    Mesh,
}

/// What one `EXECUTE_INDIRECT` does, in the core's terms.
pub(super) struct Plan {
    pub action: Action,
    pub roots: Vec<RootCopy>,
    /// Where the dispatch triple is in a record.
    pub dispatch_offset: u32,
    /// Bytes of a record.
    pub record_bytes: u32,
}

/// Checks the signature against the list's root signature `bound` and
/// works out where each argument is. Refuses what is not compute work
/// (`UNSUPPORTED`: vertex and index buffer views, rays) and
/// what does not fit the root signature (`INVALID_ARG`).
pub(super) fn plan(c: &ExecuteIndirect<'_>, bound: Option<&RootSig>) -> Result<Plan, Status> {
    let mut roots = Vec::new();
    let mut at = 0u32;
    let mut dispatch_offset = None;
    let mut action = None;
    for a in c.arguments() {
        let bytes = a.record_bytes() as u32;
        match a.kind {
            IndirectArgKind::DISPATCH
            | IndirectArgKind::DRAW
            | IndirectArgKind::DRAW_INDEXED
            | IndirectArgKind::DISPATCH_MESH => {
                dispatch_offset = Some(at);
                action = Some(match a.kind {
                    IndirectArgKind::DISPATCH => Action::Dispatch,
                    IndirectArgKind::DRAW => Action::Draw,
                    IndirectArgKind::DRAW_INDEXED => Action::DrawIndexed,
                    _ => Action::Mesh,
                });
            }
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
            // Vertex and index buffer views in a record: the table's vertex
            // buffer addresses are written by the CPU and Metal's indirect
            // indexed draw takes its index buffer when it is encoded, so a
            // record cannot change them. Rays: not done.
            _ => return Err(Status::UNSUPPORTED),
        }
        at += bytes;
    }
    let action = action.ok_or(Status::INVALID_ARG)?;
    Ok(Plan {
        action,
        roots,
        dispatch_offset: dispatch_offset.ok_or(Status::INVALID_ARG)?,
        record_bytes: at,
    })
}

/// Whether the argument is one this back does (compute, draw and mesh
/// lists; vertex and index buffer views and rays are not).
pub(super) fn is_supported_arg(kind: IndirectArgKind) -> bool {
    matches!(
        kind,
        IndirectArgKind::DISPATCH
            | IndirectArgKind::DRAW
            | IndirectArgKind::DRAW_INDEXED
            | IndirectArgKind::DISPATCH_MESH
            | IndirectArgKind::CONSTANT
            | IndirectArgKind::CONSTANT_BUFFER_VIEW
            | IndirectArgKind::SHADER_RESOURCE_VIEW
            | IndirectArgKind::UNORDERED_ACCESS_VIEW
    )
}
