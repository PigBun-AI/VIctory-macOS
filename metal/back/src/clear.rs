//! `ClearUnorderedAccessView{Float,Uint}` of textures: a compute kernel per
//! texture dimension and component type that writes one value to a region.
//!
//! Metal has no "clear this UAV" call for an arbitrary view format (a render
//! pass clear needs a render target and a load action, and only whole
//! attachments), so the clear is a dispatch. The kernel finds the texture
//! through a descriptor-table entry (the same layout a converted shader
//! reads, [`crate::descriptors::IrDescriptorTableEntry`]) in the list's
//! constant ring, and the region and value in a second block behind it. The
//! kernel bindings are the argument table's slots 29 (entry) and 30
//! (parameters), above anything a converted shader uses (bind points 0-3 and
//! the root-signature slots below them).
//!
//! # Values
//!
//! The four values are 32-bit patterns. A float, normalised or sRGB view
//! takes them as `f32` bits (`ClearUnorderedAccessViewFloat`); a `UINT` view
//! takes them as `u32`, a `SINT` view as `i32` bit patterns. Uint values given
//! to a float or normalised view are a bit pattern too; that is only defined
//! here for a 32-bit-per-channel format or for all-zero values, anything else
//! is [`Error::Unsupported`] (the original API converts per format, which no
//! game this back is for relies on). Float values given to an integer view
//! are refused the same way.

use std::collections::HashMap;
use std::sync::Mutex;

use crate::device::Device;
use crate::error::{Error, Result};
use crate::format;
use crate::pipeline::ComputePipeline;
use crate::record::CommandList;
use crate::texture::{TexDim, Texture, ViewDim, ViewSpec};

const ENTRY_SLOT: u32 = 29;
const PARAM_SLOT: u32 = 30;

/// How the kernel sees the texture.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Shape {
    D1,
    D1Array,
    D2,
    D2Array,
    D3,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Comp {
    Float,
    Uint,
    Int,
}

/// The clear kernels of a device, compiled on first use.
pub struct ClearKernels {
    device: Device,
    cache: Mutex<HashMap<(Shape, Comp), ComputePipeline>>,
}

impl Device {
    /// An empty cache of clear kernels (shared by every list that clears).
    pub fn clear_kernels(&self) -> ClearKernels {
        ClearKernels {
            device: self.clone(),
            cache: Mutex::new(HashMap::new()),
        }
    }
}

fn source(shape: Shape, comp: Comp) -> String {
    let (tex, write) = match shape {
        Shape::D1 => ("texture1d", "tex.write(v, p.x0 + t.x)"),
        Shape::D1Array => ("texture1d_array", "tex.write(v, p.x0 + t.x, p.z0 + t.z)"),
        Shape::D2 => ("texture2d", "tex.write(v, uint2(p.x0 + t.x, p.y0 + t.y))"),
        Shape::D2Array => (
            "texture2d_array",
            "tex.write(v, uint2(p.x0 + t.x, p.y0 + t.y), p.z0 + t.z)",
        ),
        Shape::D3 => (
            "texture3d",
            "tex.write(v, uint3(p.x0 + t.x, p.y0 + t.y, p.z0 + t.z))",
        ),
    };
    let (ty, conv) = match comp {
        Comp::Float => ("float", "as_type<float4>(p.bits)"),
        Comp::Uint => ("uint", "p.bits"),
        Comp::Int => ("int", "as_type<int4>(p.bits)"),
    };
    format!(
        "#include <metal_stdlib>\nusing namespace metal;\n\
struct Entry {{ ulong va; {tex}<{ty}, access::write> tex; ulong md; }};\n\
struct P {{ uint4 bits; uint x0; uint y0; uint z0; uint pad0; uint w; uint h; uint d; uint pad1; }};\n\
kernel void clear_kernel(device const Entry* e [[buffer({ENTRY_SLOT})]], \
constant P& p [[buffer({PARAM_SLOT})]], uint3 t [[thread_position_in_grid]]) {{\n\
    if (t.x >= p.w || t.y >= p.h || t.z >= p.d) return;\n\
    {tex}<{ty}, access::write> tex = e->tex;\n\
    {ty}4 v = {conv};\n\
    {write};\n}}\n"
    )
}

impl ClearKernels {
    fn pipeline(&self, shape: Shape, comp: Comp) -> Result<ComputePipeline> {
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(p) = cache.get(&(shape, comp)) {
            return Ok(p.clone());
        }
        let p = self
            .device
            .compute_pipeline_from_msl(&source(shape, comp), "clear_kernel")?;
        cache.insert((shape, comp), p.clone());
        Ok(p)
    }
}

/// What to clear.
#[derive(Clone, Copy, Debug)]
pub struct ClearRequest {
    /// DXGI number of the view format; 0 = the texture's own.
    pub format: u32,
    pub mip: u32,
    pub first_slice: u32,
    /// 0 = every slice from `first_slice`.
    pub slice_count: u32,
    /// The four values' 32-bit patterns.
    pub values: [u32; 4],
    /// The patterns are `f32` values (`ClearUnorderedAccessViewFloat`).
    pub float_values: bool,
    /// `[left, top, right, bottom]` in texels of the mip; `None` = all of it.
    pub rect: Option<[u32; 4]>,
}

impl CommandList {
    /// Clears a region of one mip of `texture` through a UAV in the format `req.format`.
    pub fn clear_texture(
        &mut self,
        kernels: &ClearKernels,
        texture: &Texture,
        req: &ClearRequest,
    ) -> Result<()> {
        self.live()?;
        let d = *texture.desc();
        let bad = |m: String| Err(Error::BadArgument(format!("clear of {d:?}: {m}")));
        if !texture.belongs_to(self.queue().device()) {
            return bad("texture of another device".into());
        }
        if d.usage & 2 == 0 {
            return bad("the texture has no UAV usage".into());
        }
        if req.mip >= d.mips {
            return bad(format!("mip {} of {}", req.mip, d.mips));
        }
        let view_format = if req.format == 0 {
            d.format
        } else {
            req.format
        };
        let f = format::require(view_format)?;
        let comp = if f.name.ends_with("_UINT") {
            Comp::Uint
        } else if f.name.ends_with("_SINT") {
            Comp::Int
        } else {
            Comp::Float
        };
        let thirty_two_bit_channels = f.bytes % 4 == 0 && f.bytes / 4 <= 4 && f.name.contains("32");
        match (comp, req.float_values) {
            (Comp::Float, false) if !thirty_two_bit_channels && req.values != [0; 4] => {
                return Err(Error::Unsupported(format!(
                    "ClearUnorderedAccessViewUint of the {} view with values other than 0 (the pattern is only defined for 32-bit channels)",
                    f.name
                )));
            }
            (Comp::Uint | Comp::Int, true) => {
                return Err(Error::Unsupported(format!(
                    "ClearUnorderedAccessViewFloat of the integer {} view",
                    f.name
                )));
            }
            _ => {}
        }
        let mip = d.mip_size(req.mip);
        let (shape, dim, slices, first_slice, z_range) = match d.dim {
            TexDim::D1 if d.array_len() == 1 => (Shape::D1, ViewDim::D1, 1, 0, (0, 1)),
            TexDim::D1 => (
                Shape::D1Array,
                ViewDim::D1Array,
                d.array_len(),
                req.first_slice,
                (0, 0),
            ),
            TexDim::D2 if d.array_len() == 1 && d.samples == 1 => {
                (Shape::D2, ViewDim::D2, 1, 0, (0, 1))
            }
            TexDim::D2 if d.samples == 1 => (
                Shape::D2Array,
                ViewDim::D2Array,
                d.array_len(),
                req.first_slice,
                (0, 0),
            ),
            TexDim::D3 => (Shape::D3, ViewDim::D3, 1, 0, (0, mip[2])),
            _ => return Err(Error::Unsupported("clearing a multisampled texture".into())),
        };
        let (slice_first, count_of_slices, z0, zn) =
            if matches!(shape, Shape::D1Array | Shape::D2Array) {
                let count = if req.slice_count == 0 {
                    slices.saturating_sub(first_slice)
                } else {
                    req.slice_count
                };
                if first_slice >= slices || count == 0 || first_slice + count > slices {
                    return bad(format!("slices {first_slice}+{count} of {slices}"));
                }
                (first_slice, count, 0, count)
            } else {
                (0, 1, z_range.0, z_range.1)
            };
        let rect = req.rect.unwrap_or([0, 0, mip[0], mip[1]]);
        let one_row = matches!(shape, Shape::D1 | Shape::D1Array);
        let (top, bottom) = if one_row { (0, 1) } else { (rect[1], rect[3]) };
        if rect[0] >= rect[2] || rect[2] > mip[0] || top >= bottom || bottom > mip[1] {
            return bad(format!(
                "rectangle {rect:?} outside the {}x{} mip",
                mip[0], mip[1]
            ));
        }
        let (w, h, y0) = (rect[2] - rect[0], bottom - top, top);
        let mut spec = ViewSpec::whole(dim);
        spec.format = req.format;
        spec.mip_first = req.mip;
        spec.mip_count = 1;
        spec.slice_first = slice_first;
        spec.slice_count = if shape == Shape::D3 {
            0
        } else {
            count_of_slices
        };
        // A view of the wrong sample type for the kernel would write nothing
        // useful; `view` refuses formats of another family.
        let view_id = texture.view(&spec)?;
        let pso = kernels.pipeline(shape, comp)?;

        // Entry (24 bytes, as a converted shader reads a texture descriptor)
        // at 0, parameters at 32: bits, origin, extent.
        let ring = self.alloc_constants(96, 16)?;
        let mut entry = [0u8; 24];
        entry[8..16].copy_from_slice(&view_id.to_le_bytes());
        ring.write_bytes(0, &entry)?;
        ring.write_u32s(
            32,
            &[
                req.values[0],
                req.values[1],
                req.values[2],
                req.values[3],
                rect[0],
                y0,
                z0,
                0,
                w,
                h,
                zn,
                0,
            ],
        )?;
        self.keep_textures_for_clear(texture);
        self.set_address(ENTRY_SLOT, ring.gpu_va)?;
        self.set_address(PARAM_SLOT, ring.gpu_va + 32)?;
        let threads = match shape {
            Shape::D1 | Shape::D1Array => [64, 1, 1],
            _ => [8, 8, 1],
        };
        let groups = [w.div_ceil(threads[0]), h.div_ceil(threads[1]), zn];
        self.dispatch(&pso, groups, threads)
    }

    fn keep_textures_for_clear(&mut self, t: &Texture) {
        self.copies.keep_texture(t);
    }
}
