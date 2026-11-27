//! `ID3D12CommandSignature` and `ExecuteIndirect`.
//!
//! A signature is checked when it is made, the way the runtime does (an
//! action argument last and alone, a root signature when an argument changes
//! root arguments, a stride that holds the record, root parameters of the
//! right kind), and then lives only here: `ExecuteIndirect` sends the whole
//! signature in the command (`victory_metal_proto::indirect`). Draw, mesh
//! dispatch and vertex / index buffer arguments are accepted here (they are
//! valid signatures) and sent as they are; the back answers `UNSUPPORTED`
//! until the render encoder is in, and the list reports it at `Close`.

use victory_metal_proto::cmd::{Command, MAX_ROOT_CONSTANTS};
use victory_metal_proto::ids::{ResId, RootSigId};
use victory_metal_proto::indirect::{
    ExecuteIndirect, IndirectArg, IndirectArgKind, MAX_INDIRECT_ARGS,
};

use crate::device::{Buffer, CommandList, Item, RootSignature};
use crate::hresult::{Hr, Result};
use crate::rootsig::{self, ParamKind};

/// One `D3D12_INDIRECT_ARGUMENT_DESC`: the type and the union's fields
/// (`slot` / `root_parameter_index` in `index`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ArgDesc {
    pub ty: u32,
    pub index: u32,
    pub dest_offset: u32,
    pub count: u32,
}

/// A command signature, checked.
#[derive(Clone, Debug)]
pub struct CommandSignature {
    pub stride: u32,
    pub args: Vec<IndirectArg>,
    /// The root signature, when an argument changes root arguments.
    pub root_sig: RootSigId,
    /// Bytes of a record.
    pub record_bytes: u32,
}

impl CommandSignature {
    /// `CreateCommandSignature`.
    pub fn new(stride: u32, descs: &[ArgDesc], root: Option<&RootSignature>) -> Result<Self> {
        if descs.is_empty() || descs.len() > MAX_INDIRECT_ARGS {
            return Err(Hr::E_INVALIDARG);
        }
        let mut args = Vec::with_capacity(descs.len());
        let mut bytes = 0u64;
        let mut sets_root = false;
        for (i, d) in descs.iter().enumerate() {
            let kind = IndirectArgKind(d.ty);
            if !kind.is_known() {
                return Err(Hr::E_INVALIDARG);
            }
            // The action is the last argument and the only one.
            if kind.is_action() != (i + 1 == descs.len()) {
                return Err(Hr::E_INVALIDARG);
            }
            let arg = match kind {
                IndirectArgKind::CONSTANT => IndirectArg {
                    kind,
                    index: d.index,
                    dest_offset: d.dest_offset,
                    count: d.count,
                },
                IndirectArgKind::VERTEX_BUFFER_VIEW => IndirectArg {
                    kind,
                    index: d.index,
                    dest_offset: 0,
                    count: 0,
                },
                IndirectArgKind::CONSTANT_BUFFER_VIEW
                | IndirectArgKind::SHADER_RESOURCE_VIEW
                | IndirectArgKind::UNORDERED_ACCESS_VIEW => IndirectArg {
                    kind,
                    index: d.index,
                    dest_offset: 0,
                    count: 0,
                },
                _ => IndirectArg {
                    kind,
                    index: 0,
                    dest_offset: 0,
                    count: 0,
                },
            };
            if kind == IndirectArgKind::CONSTANT
                && (d.count == 0 || d.count as usize > MAX_ROOT_CONSTANTS)
            {
                return Err(Hr::E_INVALIDARG);
            }
            if kind.sets_root() {
                sets_root = true;
                let rs = root.ok_or(Hr::E_INVALIDARG)?;
                check_root_param(rs, &arg)?;
            }
            bytes += arg.record_bytes();
            args.push(arg);
        }
        if u64::from(stride) < bytes || !stride.is_multiple_of(4) {
            return Err(Hr::E_INVALIDARG);
        }
        Ok(CommandSignature {
            stride,
            args,
            root_sig: match (sets_root, root) {
                (true, Some(rs)) => rs.id,
                _ => RootSigId::NULL,
            },
            record_bytes: bytes as u32,
        })
    }
}

/// The root parameter an argument names must exist and be of the kind it sets.
fn check_root_param(rs: &RootSignature, a: &IndirectArg) -> Result<()> {
    let p = rs
        .desc
        .params
        .get(a.index as usize)
        .ok_or(Hr::E_INVALIDARG)?;
    let want = |ty: u32| matches!(p.kind, ParamKind::Descriptor { ty: t, .. } if t == ty);
    let ok = match a.kind {
        IndirectArgKind::CONSTANT => match p.kind {
            ParamKind::Constants { num, .. } => {
                u64::from(a.dest_offset) + u64::from(a.count) <= u64::from(num)
            }
            _ => false,
        },
        IndirectArgKind::CONSTANT_BUFFER_VIEW => want(rootsig::PARAM_CBV),
        IndirectArgKind::SHADER_RESOURCE_VIEW => want(rootsig::PARAM_SRV),
        IndirectArgKind::UNORDERED_ACCESS_VIEW => want(rootsig::PARAM_UAV),
        _ => false,
    };
    if ok { Ok(()) } else { Err(Hr::E_INVALIDARG) }
}

impl CommandList {
    /// `ExecuteIndirect`. `args` must be in the indirect-argument state (the
    /// application's barriers say so); a count buffer, when given, is read
    /// at `count.1` and caps `max_count`.
    pub fn execute_indirect(
        &mut self,
        sig: &CommandSignature,
        max_count: u32,
        args: &Buffer,
        args_offset: u64,
        count: Option<(&Buffer, u64)>,
    ) {
        if !args.desc.is_buffer()
            || !args_offset.is_multiple_of(4)
            || count.is_some_and(|(b, off)| !b.desc.is_buffer() || !off.is_multiple_of(4))
        {
            return self.fail(Hr::E_INVALIDARG);
        }
        if max_count == 0 {
            return;
        }
        // The records lie in the buffer (with a count buffer the count may
        // stop short of `max_count`, and the back cuts it to what fits).
        let last = u64::from(max_count - 1) * u64::from(sig.stride) + u64::from(sig.record_bytes);
        if count.is_none()
            && args_offset
                .checked_add(last)
                .is_none_or(|e| e > args.desc.width)
        {
            return self.fail(Hr::E_INVALIDARG);
        }
        if args_offset
            .checked_add(u64::from(sig.record_bytes))
            .is_none_or(|e| e > args.desc.width)
            || count.is_some_and(|(b, off)| off.checked_add(4).is_none_or(|e| e > b.desc.width))
        {
            return self.fail(Hr::E_INVALIDARG);
        }
        let mut signature = Vec::new();
        ExecuteIndirect::encode_signature(&sig.args, &mut signature);
        self.push_item(Item::Indirect(IndirectItem {
            args: args.res,
            args_offset,
            count: count.map_or(ResId::NULL, |(b, _)| b.res),
            count_offset: count.map_or(0, |(_, off)| off),
            max_count,
            stride: sig.stride,
            root_sig: sig.root_sig,
            signature,
        }));
    }
}

/// `ExecuteIndirect` as the list holds it until the block is encoded: the
/// signature's bytes are owned here.
#[derive(Clone, Debug, PartialEq)]
pub struct IndirectItem {
    pub args: ResId,
    pub args_offset: u64,
    pub count: ResId,
    pub count_offset: u64,
    pub max_count: u32,
    pub stride: u32,
    pub root_sig: RootSigId,
    pub signature: Vec<u8>,
}

impl IndirectItem {
    /// The command, borrowing the signature bytes.
    pub fn command(&self) -> Command<'_> {
        Command::ExecuteIndirect(ExecuteIndirect {
            args: self.args,
            args_offset: self.args_offset,
            count: self.count,
            count_offset: self.count_offset,
            max_count: self.max_count,
            stride: self.stride,
            root_sig: self.root_sig,
            signature: &self.signature,
        })
    }
}
