//! Three-channel 32-bit typed views read through an expanded mirror: the compute kernel that
//! makes one 16-byte element (`x y z w`) of each 12-byte element (`x y z`) of a buffer.
//!
//! `R32G32B32_FLOAT/UINT/SINT` have a Metal pixel format only from macOS 27.0 and not on every
//! device (see [`crate::typed`]); a four-channel format has always been there. The expansion is
//! a plain kernel over device addresses: the source address and the mirror address in the first
//! constant block, the element count and the value of the fourth channel (1.0 or 1, what D3D12
//! returns for a channel the format lacks) in the second. The kernel binds like the clear
//! kernels (`clear.rs`): argument table slots 29 and 30, above anything a converted shader
//! uses.

use std::sync::OnceLock;

use crate::error::Result;
use crate::memory::Buffer;
use crate::pipeline::ComputePipeline;
use crate::record::CommandList;

const ADDR_SLOT: u32 = 29;
const PARAM_SLOT: u32 = 30;
const THREADS: u32 = 64;

const SOURCE: &str = "#include <metal_stdlib>\nusing namespace metal;\n\
struct A { ulong src; ulong dst; };\n\
struct P { uint count; uint fourth; };\n\
kernel void expand_rgb32(device const A* a [[buffer(29)]], constant P& p [[buffer(30)]], \
uint i [[thread_position_in_grid]]) {\n\
    if (i >= p.count) return;\n\
    device const uint* s = reinterpret_cast<device const uint*>(a->src);\n\
    device uint* d = reinterpret_cast<device uint*>(a->dst);\n\
    d[i * 4 + 0] = s[i * 3 + 0];\n\
    d[i * 4 + 1] = s[i * 3 + 1];\n\
    d[i * 4 + 2] = s[i * 3 + 2];\n\
    d[i * 4 + 3] = p.fourth;\n}\n";

fn pipeline(device: &crate::device::Device) -> Result<ComputePipeline> {
    // One kernel per process and device: devices are not mixed in one process of this back.
    static KERNEL: OnceLock<ComputePipeline> = OnceLock::new();
    if let Some(p) = KERNEL.get() {
        return Ok(p.clone());
    }
    let p = device.compute_pipeline_from_msl(SOURCE, "expand_rgb32")?;
    Ok(KERNEL.get_or_init(|| p).clone())
}

impl CommandList {
    /// Writes `count` four-channel elements into `mirror` from the `count` three-channel
    /// elements at byte `offset` of `src`; the fourth channel is `fourth` (bits).
    pub(crate) fn expand_three_channel(
        &mut self,
        mirror: &Buffer,
        src: &Buffer,
        offset: u64,
        count: u64,
        fourth: u32,
    ) -> Result<()> {
        self.live()?;
        self.check_buffer(mirror, 0, count * 16, "expanded mirror")?;
        self.check_buffer(src, offset, count * 12, "expanded source")?;
        let pso = pipeline(self.queue().device())?;
        let ring = self.alloc_constants(48, 16)?;
        ring.write_bytes(0, &(src.gpu_va() + offset).to_le_bytes())?;
        ring.write_bytes(8, &mirror.gpu_va().to_le_bytes())?;
        ring.write_u32s(32, &[count as u32, fourth])?;
        self.keep_buffers.push(mirror.clone());
        self.keep_buffers.push(src.clone());
        self.set_address(ADDR_SLOT, ring.gpu_va)?;
        self.set_address(PARAM_SLOT, ring.gpu_va + 32)?;
        self.dispatch(
            &pso,
            [(count as u32).div_ceil(THREADS), 1, 1],
            [THREADS, 1, 1],
        )
    }
}
