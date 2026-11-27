//! `ExecuteIndirect` of compute work.
//!
//! # How a record becomes work
//!
//! A command signature is a list of arguments; a record in the argument
//! buffer holds them one after the other, and the last is the action. For a
//! compute list the action is a dispatch, the others set root arguments
//! (32-bit constants, root CBV/SRV/UAV addresses). Three cases:
//!
//! * **Dispatch alone, no count buffer.** The record *is* Metal's
//!   `MTLDispatchThreadgroupsIndirectArguments` (three `u32` at its start):
//!   one `dispatchThreadgroupsWithIndirectBuffer` per record, straight from
//!   the application's buffer. The cheapest case and the one titles use for
//!   culling and tile passes.
//! * **A count buffer, or root arguments.** A small kernel
//!   (`expand`, [`Kernels`]) reads the records on the GPU and, per record,
//!   writes (1) a dispatch triple, zero when the record is past the count, and
//!   (2) when the signature changes root arguments, a copy of the list's
//!   top-level argument buffer with the record's values written over the
//!   parameters' slots. Then one indirect dispatch per record, each with
//!   its own argument buffer bound.
//!
//! Why a kernel and not a `MTLIndirectCommandBuffer`: the converted shaders
//! read their root arguments from one argument buffer at a fixed slot
//! ([`crate::descriptors::bind_point`]); a record only has to change bytes in
//! it, which a kernel does on any pipeline. An indirect command buffer needs
//! the pipelines made with indirect-command support and a kernel to encode
//! the commands anyway; this back's pipelines (converted by MSC, created by
//! `create_compute_pipeline`) are not, and a second variant of each would
//! double the pipeline count for a feature a few passes use. The cost is one
//! encoded dispatch per `MaxCommandCount` (an indirect dispatch of zero
//! groups for a record past the count), which is why the count is limited
//! ([`MAX_RECORDS`]).
//!
//! The count and arguments are read when the list *runs* (after the
//! application's barriers), not when it is recorded.

use std::sync::Mutex;

use crate::device::Device;
use crate::error::{Error, Result};
use crate::memory::Buffer;
use crate::pipeline::ComputePipeline;
use crate::record::CommandList;
use crate::stages::{DISPATCH, VIS_DEVICE};

/// The argument table slot the kernel reads its parameters from (above the
/// slots a converted shader uses, like [`crate::clear`]'s).
const PARAM_SLOT: u32 = 30;
/// Records one expansion handles: a ring allocation's worth.
const CHUNK: u32 = 256;
/// Most records one `ExecuteIndirect` may have (`MaxCommandCount`): each is
/// an encoded dispatch.
pub const MAX_RECORDS: u32 = 1 << 16;

const SOURCE: &str = "#include <metal_stdlib>\nusing namespace metal;\n\
struct P { ulong args_va; ulong count_va; ulong base_va; ulong out_tlab_va; ulong out_disp_va; ulong desc_va;\n\
  uint n; uint first; uint stride; uint tlab_words; uint tlab_stride_words; uint ndesc; uint disp_word; uint max_count;\n\
  uint has_count; uint act_words; uint out_words; uint index_type; };\n\
struct D { uint rec_word; uint tlab_word; uint words; uint pad; };\n\
kernel void expand(constant P& p [[buffer(30)]], uint i [[thread_position_in_grid]]) {\n\
  if (i >= p.n) return;\n\
  uint idx = p.first + i;\n\
  uint ow = p.out_words;\n\
  uint aw = p.act_words;\n\
  device uint* disp = ((device uint*)p.out_disp_va) + (ulong)i * (ulong)ow;\n\
  bool live = idx < p.max_count;\n\
  if (p.has_count != 0) { live = live && idx < *(device const uint*)p.count_va; }\n\
  for (uint k = 0; k < ow; k++) { disp[k] = 0; }\n\
  if (ow >= 16) { disp[8] = p.index_type; }\n\
  if (!live) { return; }\n\
  device const uint* rec = (device const uint*)(p.args_va + (ulong)idx * (ulong)p.stride);\n\
  if (p.ndesc != 0) {\n\
    device uint* t = ((device uint*)p.out_tlab_va) + (ulong)i * (ulong)p.tlab_stride_words;\n\
    device const uint* base = (device const uint*)p.base_va;\n\
    for (uint w = 0; w < p.tlab_words; w++) { t[w] = base[w]; }\n\
    device const D* d = (device const D*)p.desc_va;\n\
    for (uint k = 0; k < p.ndesc; k++) {\n\
      for (uint w = 0; w < d[k].words; w++) { t[d[k].tlab_word + w] = rec[d[k].rec_word + w]; }\n\
    }\n\
  }\n\
  for (uint k = 0; k < aw; k++) { disp[k] = rec[p.disp_word + k]; }\n\
}\n";

/// The expansion kernel of a device, compiled on first use.
pub struct Kernels {
    device: Device,
    expand: Mutex<Option<ComputePipeline>>,
}

impl Device {
    /// An empty holder of the `ExecuteIndirect` kernel.
    pub fn indirect_kernels(&self) -> Kernels {
        Kernels {
            device: self.clone(),
            expand: Mutex::new(None),
        }
    }
}

impl Kernels {
    fn expand(&self) -> Result<ComputePipeline> {
        let mut g = self.expand.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(p) = &*g {
            return Ok(p.clone());
        }
        let p = self.device.compute_pipeline_from_msl(SOURCE, "expand")?;
        *g = Some(p.clone());
        Ok(p)
    }
}

/// One root argument a record sets: `words` 32-bit values from `rec_word`
/// of the record go to `tlab_word` of the argument buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RootCopy {
    pub rec_word: u32,
    pub tlab_word: u32,
    pub words: u32,
}

/// An `ExecuteIndirect` of dispatches.
#[derive(Clone, Copy)]
pub struct IndirectDispatch<'a> {
    pub args: &'a Buffer,
    pub args_offset: u64,
    /// The buffer and offset of the 32-bit record count.
    pub count: Option<(&'a Buffer, u64)>,
    pub max_count: u32,
    pub stride: u32,
    /// Where the dispatch triple is in a record, in bytes.
    pub dispatch_offset: u32,
    /// Bytes one record takes up to its end (the last argument's end).
    pub record_bytes: u32,
    /// The root arguments a record sets.
    pub roots: &'a [RootCopy],
    /// The list's argument buffer as set so far (what a record starts from).
    pub base: &'a [u8],
    /// 32-bit words of the action in a record: 3 for a dispatch or a mesh
    /// dispatch, 4 for a draw, 5 for an indexed draw.
    pub action_words: u32,
    /// The action is a vertex-pipeline draw: the expansion also writes the
    /// converter's draw parameters (a copy of the draw's words, then the
    /// index type at +32 bytes), so the kernel is always used.
    pub draws: bool,
    /// The converter runtime's index type (`IRMetalIndexToIRIndex`) of the
    /// bound index buffer, for an indexed draw.
    pub index_type: u32,
    /// Stages that read what the expansion wrote.
    pub consumer: u16,
}

/// One record as the encoder of the action sees it.
#[derive(Clone, Copy, Debug)]
pub struct Record {
    /// Where the action's arguments are: the application's record itself,
    /// or, after an expansion, the expanded copy (for a draw, its first
    /// 32 bytes are the arguments, the next the converter's uniforms).
    pub action_va: u64,
    /// The argument buffer to bind for this record, when its own copy was made.
    pub tlab_va: Option<u64>,
}

impl IndirectDispatch<'_> {
    /// Whether the kernel is needed (otherwise the buffer is dispatched from as it is).
    pub fn needs_kernel(&self) -> bool {
        self.count.is_some() || !self.roots.is_empty() || self.draws
    }

    /// Whether the list's argument buffer is replaced by a copy per record.
    pub fn replaces_root_arguments(&self) -> bool {
        !self.roots.is_empty()
    }
}

impl CommandList {
    /// A dispatch whose group counts the GPU reads at `args_va` (4-byte
    /// aligned): `{x, y, z}` as `u32`.
    pub fn dispatch_indirect(
        &mut self,
        pipeline: &ComputePipeline,
        args_va: u64,
        threads: [u32; 3],
    ) -> Result<()> {
        self.encode_dispatch_indirect(pipeline, args_va, threads)
    }

    /// `ExecuteIndirect` with `spec`: up to `max_count` dispatches of `pipeline`.
    pub fn execute_indirect(
        &mut self,
        kernels: &Kernels,
        pipeline: &ComputePipeline,
        threads: [u32; 3],
        spec: &IndirectDispatch<'_>,
    ) -> Result<()> {
        self.execute_indirect_with(kernels, spec, |l, r| {
            if let Some(t) = r.tlab_va {
                l.set_root_arguments_va(t)?;
            }
            l.encode_dispatch_indirect(pipeline, r.action_va, threads)
        })
    }

    /// The records of `spec`, each handed to `encode` after the list's
    /// argument table has been pointed at what the record needs.
    pub(crate) fn execute_indirect_with(
        &mut self,
        kernels: &Kernels,
        spec: &IndirectDispatch<'_>,
        mut encode: impl FnMut(&mut CommandList, Record) -> Result<()>,
    ) -> Result<()> {
        self.live()?;
        if spec.max_count == 0 {
            return Ok(());
        }
        // Records that lie inside the buffer. With a count buffer the count
        // decides how many are read and a `MaxCommandCount` larger than the
        // buffer is a bound, not a promise: only records that fit are run.
        // Without one, every record must fit.
        let room = spec.args.len().saturating_sub(spec.args_offset);
        let fit = match room.checked_sub(u64::from(spec.record_bytes)) {
            Some(rest) => rest / u64::from(spec.stride.max(1)) + 1,
            None => 0,
        };
        let max_count = if spec.count.is_some() {
            u64::from(spec.max_count).min(fit)
        } else {
            u64::from(spec.max_count)
        };
        if max_count == 0 {
            // Nothing can be read; say so if the arguments are not even there.
            return self.check_buffer(
                spec.args,
                spec.args_offset,
                u64::from(spec.record_bytes),
                "ExecuteIndirect arguments",
            );
        }
        if max_count > u64::from(MAX_RECORDS) {
            return Err(Error::Unsupported(format!(
                "ExecuteIndirect with MaxCommandCount {max_count} (more than {MAX_RECORDS})"
            )));
        }
        let max_count = max_count as u32;
        let spec = &IndirectDispatch { max_count, ..*spec };
        let n = u64::from(spec.max_count);
        let span = (n - 1) * u64::from(spec.stride) + u64::from(spec.record_bytes);
        self.check_buffer(
            spec.args,
            spec.args_offset,
            span,
            "ExecuteIndirect arguments",
        )?;
        if let Some((b, off)) = spec.count {
            self.check_buffer(b, off, 4, "ExecuteIndirect count")?;
        }
        if !spec.needs_kernel() {
            let base = spec.args.gpu_va() + spec.args_offset + u64::from(spec.dispatch_offset);
            for i in 0..u64::from(spec.max_count) {
                encode(
                    self,
                    Record {
                        action_va: base + i * u64::from(spec.stride),
                        tlab_va: None,
                    },
                )?;
            }
            self.keep_buffers.push(spec.args.clone());
            return Ok(());
        }

        let expand = kernels.expand()?;
        let replaces = spec.replaces_root_arguments();
        let tlab_bytes = if replaces {
            (spec.base.len() as u64).next_multiple_of(16)
        } else {
            0
        };
        let descs = spec.roots.len() as u64;
        // Ring bytes of a chunk of `k` records.
        let fixed = 96 + (16 * descs).next_multiple_of(16) + tlab_bytes;
        let out_bytes: u64 = if spec.draws { 64 } else { 16 };
        let per_record = tlab_bytes + out_bytes;
        let room = self.queue().constant_segment_bytes();
        let chunk = ((room.saturating_sub(fixed)) / per_record).min(u64::from(CHUNK));
        if chunk == 0 {
            return Err(Error::Unsupported(format!(
                "ExecuteIndirect: a root argument buffer of {} bytes does not fit the constant ring",
                spec.base.len()
            )));
        }
        let count_va = spec.count.map_or(0, |(b, off)| b.gpu_va() + off);
        let mut first = 0u64;
        while first < n {
            let k = chunk.min(n - first);
            let ring = self.alloc_constants(fixed + k * per_record, 16)?;
            let desc_at = 96u64;
            let base_at = desc_at + (16 * descs).next_multiple_of(16);
            let tlabs_at = base_at + tlab_bytes;
            let disps_at = tlabs_at + k * tlab_bytes;
            let va = |at: u64| ring.gpu_va + at;
            let mut p = Vec::<u8>::with_capacity(96);
            for v in [
                spec.args.gpu_va() + spec.args_offset,
                count_va,
                va(base_at),
                va(tlabs_at),
                va(disps_at),
                va(desc_at),
            ] {
                p.extend_from_slice(&v.to_le_bytes());
            }
            for v in [
                k as u32,
                first as u32,
                spec.stride,
                (spec.base.len() / 4) as u32 * u32::from(replaces),
                (tlab_bytes / 4) as u32,
                spec.roots.len() as u32,
                spec.dispatch_offset / 4,
                spec.max_count,
                u32::from(spec.count.is_some()),
                spec.action_words,
                (out_bytes / 4) as u32,
                spec.index_type,
            ] {
                p.extend_from_slice(&v.to_le_bytes());
            }
            ring.write_bytes(0, &p)?;
            for (i, r) in spec.roots.iter().enumerate() {
                ring.write_u32s(
                    desc_at + 16 * i as u64,
                    &[r.rec_word, r.tlab_word, r.words, 0],
                )?;
            }
            if replaces {
                ring.write_bytes(base_at, spec.base)?;
            }
            self.set_address(PARAM_SLOT, ring.gpu_va)?;
            self.dispatch(&expand, [k.div_ceil(64) as u32, 1, 1], [64, 1, 1])?;
            // The records' dispatch arguments and argument buffers are read next.
            self.barrier(DISPATCH, spec.consumer, VIS_DEVICE)?;
            for i in 0..k {
                encode(
                    self,
                    Record {
                        action_va: va(disps_at + i * out_bytes),
                        tlab_va: replaces.then(|| va(tlabs_at + i * tlab_bytes)),
                    },
                )?;
            }
            first += k;
        }
        self.keep_buffers.push(spec.args.clone());
        if let Some((b, _)) = spec.count {
            self.keep_buffers.push(b.clone());
        }
        Ok(())
    }
}
