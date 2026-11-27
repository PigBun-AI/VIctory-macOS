//! Texture copies: buffer footprint to texture, texture to buffer, texture to
//! texture, and a whole resource to another of the same description.
//!
//! They are `MTL4ComputeCommandEncoder` copies, in the `BLIT` stage like the
//! buffer copies of [`crate::record`]. Metal refuses nothing in a release
//! build and does something undefined in a debug one, so every argument
//! the original API would call an error is checked here and is an
//! [`Error`], never a silent clamp or a wrong copy:
//!
//! * the region lies inside the mip level, and inside the buffer
//!   (`offset + last row` reaches no further than its length);
//! * the row pitch is at least one row of the region and a multiple of the
//!   texel size (of the block size for compressed formats): what Metal needs
//!   of `bytesPerRow`. The original API's 256-byte pitch rule is the
//!   front's, which never sends a smaller one;
//! * the buffer offset is a multiple of the texel (block) size;
//! * block-compressed formats are counted in 4x4 blocks: the region starts on a
//!   block and ends on one or on the edge of the mip;
//! * the footprint format has the texel size and block shape of the texture's
//!   plane;
//! * depth: a D24 texture's depth plane is refused (the texture holds 32-bit
//!   floats, see [`crate::format`]); stencil planes copy as bytes, a 32-bit
//!   float depth plane as floats.
//!
//! Hazards: two copies that touch the same bytes of a buffer or the same
//! subresource of a texture, one of them writing, are separated by a `BLIT`
//! barrier the backend inserts itself (the original API runs copies in
//! order); copies of different parts overlap.

use objc2_metal::{MTL4ComputeCommandEncoder, MTLBlitOption, MTLOrigin, MTLSize};

use crate::error::{Error, Result};
use crate::format::{self, Format};
use crate::memory::Buffer;
use crate::record::CommandList;
use crate::stages::BLIT;
use crate::texture::{TexDim, Texture};

/// A subresource of a texture and a corner in it.
#[derive(Clone, Copy)]
pub struct TexLoc<'a> {
    pub texture: &'a Texture,
    pub mip: u32,
    /// Array slice (0 for a 3D texture).
    pub slice: u32,
    /// 0 depth or colour, 1 stencil.
    pub plane: u32,
    /// Corner of the region, in texels.
    pub origin: [u32; 3],
}

/// Bytes of a buffer laid out as an image.
#[derive(Clone, Copy)]
pub struct BufLoc<'a> {
    pub buffer: &'a Buffer,
    /// Byte offset of the first texel of the region.
    pub offset: u64,
    /// DXGI number of the texels in the buffer.
    pub format: u32,
    /// Bytes between rows.
    pub row_pitch: u32,
    /// Rows in one image (a 3D region's slices are `row_pitch * rows` apart);
    /// at least the region's height when it has more than one slice.
    pub rows: u32,
}

/// What a list's texture copies keep alive and which subresources they
/// touched since the last barrier between copies.
#[derive(Default)]
pub struct CopyState {
    keep: Vec<Texture>,
    /// `(texture, mip, slice, written)`; `u32::MAX` = every mip / slice.
    pub(crate) accesses: Vec<(Texture, u32, u32, bool)>,
}

impl CopyState {
    /// Keeps `t` alive until the list is done.
    pub(crate) fn keep_texture(&mut self, t: &Texture) {
        self.keep.push(t.clone());
    }
}

const ALL: u32 = u32::MAX;

/// Layout of one plane of a texture format: bytes per block, block shape,
/// and the blit option that picks the plane out of a depth-stencil texture.
struct PlaneLayout {
    bytes: u8,
    block: (u8, u8),
    option: MTLBlitOption,
}

fn plane_layout(f: &Format, plane: u32) -> Result<PlaneLayout> {
    if plane >= f.planes() {
        return Err(Error::BadArgument(format!(
            "plane {plane} of {} ({} planes)",
            f.name,
            f.planes()
        )));
    }
    if f.planes() == 1 {
        return Ok(PlaneLayout {
            bytes: f.bytes,
            block: f.block,
            option: MTLBlitOption::None,
        });
    }
    if plane == 0 {
        if matches!(f.family, format::dxgi::R24G8_TYPELESS) {
            return Err(Error::Unsupported(format!(
                "copying the depth plane of {}: the texture holds 32-bit floats, the buffer layout is 24-bit fixed point",
                f.name
            )));
        }
        Ok(PlaneLayout {
            bytes: 4,
            block: (1, 1),
            option: MTLBlitOption::DepthFromDepthStencil,
        })
    } else {
        Ok(PlaneLayout {
            bytes: 1,
            block: (1, 1),
            option: MTLBlitOption::StencilFromDepthStencil,
        })
    }
}

/// Checks a texture region and returns the plane layout, the format row and
/// the region in blocks-aligned Metal terms (`size` is returned unchanged;
/// for compressed formats a region that reaches the mip's edge is rounded up
/// to whole blocks, which is what Metal wants).
fn check_texture(
    list: &CommandList,
    loc: &TexLoc<'_>,
    size: [u32; 3],
    what: &str,
) -> Result<(&'static Format, PlaneLayout, [u32; 3])> {
    let t = loc.texture;
    if !t.belongs_to(list.queue().device()) {
        return Err(Error::BadArgument(format!(
            "{what}: texture of another device"
        )));
    }
    let d = t.desc();
    let f = format::require(d.format)?;
    let layout = plane_layout(f, loc.plane)?;
    if loc.mip >= d.mips {
        return Err(Error::BadArgument(format!(
            "{what}: mip {} of a texture with {}",
            loc.mip, d.mips
        )));
    }
    if loc.slice >= d.array_len() {
        return Err(Error::BadArgument(format!(
            "{what}: slice {} of a texture with {}",
            loc.slice,
            d.array_len()
        )));
    }
    if d.samples > 1 {
        return Err(Error::Unsupported(format!(
            "{what}: region copies of a multisampled texture"
        )));
    }
    let mip = d.mip_size(loc.mip);
    let (bw, bh) = (u32::from(layout.block.0), u32::from(layout.block.1));
    let mut size = size;
    for axis in 0..3 {
        let block = match axis {
            0 => bw,
            1 => bh,
            _ => 1,
        };
        let (o, s) = (loc.origin[axis], size[axis]);
        let end = o
            .checked_add(s)
            .ok_or_else(|| Error::BadArgument(format!("{what}: region overflows")))?;
        if end > mip[axis] {
            return Err(Error::BadArgument(format!(
                "{what}: region {o}+{s} on axis {axis} is outside mip {} of {}",
                loc.mip, mip[axis]
            )));
        }
        if block > 1 {
            if o % block != 0 {
                return Err(Error::BadArgument(format!(
                    "{what}: origin {o} on axis {axis} is not on a {block}-texel block"
                )));
            }
            if s % block != 0 {
                if end == mip[axis] {
                    size[axis] = s
                        .next_multiple_of(block)
                        .min(mip[axis].next_multiple_of(block));
                } else {
                    return Err(Error::BadArgument(format!(
                        "{what}: size {s} on axis {axis} is not a whole number of {block}-texel blocks"
                    )));
                }
            }
        }
    }
    if d.dim != TexDim::D3 && (loc.origin[2] != 0 || size[2] > 1) {
        return Err(Error::BadArgument(format!(
            "{what}: a depth range of a texture that is not 3D"
        )));
    }
    Ok((f, layout, size))
}

/// Checks the buffer side of a copy of `size` texels laid out per `layout`;
/// returns bytes per image.
fn check_buffer(
    list: &CommandList,
    b: &BufLoc<'_>,
    layout: &PlaneLayout,
    size: [u32; 3],
    what: &str,
) -> Result<(u64, u64)> {
    let bf = format::require(b.format)?;
    if bf.bytes != layout.bytes || bf.block != layout.block {
        return Err(Error::BadArgument(format!(
            "{what}: footprint format {} is {} bytes per {:?} block, the texture plane is {} per {:?}",
            bf.name, bf.bytes, bf.block, layout.bytes, layout.block
        )));
    }
    let row_bytes = bf.row_bytes(size[0]);
    let rows = u64::from(bf.block_rows(size[1]));
    let bytes = u64::from(layout.bytes);
    if u64::from(b.row_pitch) < row_bytes {
        return Err(Error::BadArgument(format!(
            "{what}: row pitch {} is less than a row of the region ({row_bytes} bytes)",
            b.row_pitch
        )));
    }
    if !u64::from(b.row_pitch).is_multiple_of(bytes) {
        return Err(Error::BadArgument(format!(
            "{what}: row pitch {} is not a multiple of the {bytes}-byte texel (block)",
            b.row_pitch
        )));
    }
    if !b.offset.is_multiple_of(bytes) {
        return Err(Error::BadArgument(format!(
            "{what}: buffer offset {} is not a multiple of the {bytes}-byte texel (block)",
            b.offset
        )));
    }
    let image_rows = u64::from(bf.block_rows(b.rows.max(size[1])));
    if size[2] > 1 && u64::from(bf.block_rows(b.rows)) < rows {
        return Err(Error::BadArgument(format!(
            "{what}: {} rows per image for a region {} rows high",
            b.rows, size[1]
        )));
    }
    let per_image = u64::from(b.row_pitch) * image_rows;
    let last = b.offset
        + u64::from(size[2].saturating_sub(1)) * per_image
        + rows.saturating_sub(1) * u64::from(b.row_pitch)
        + row_bytes;
    let extent = last - b.offset;
    list.check_buffer(b.buffer, b.offset, extent, what)?;
    Ok((per_image, extent))
}

fn mtl_origin(loc: &TexLoc<'_>) -> MTLOrigin {
    MTLOrigin {
        x: loc.origin[0] as usize,
        y: loc.origin[1] as usize,
        z: loc.origin[2] as usize,
    }
}

fn mtl_size(size: [u32; 3]) -> MTLSize {
    MTLSize {
        width: size[0] as usize,
        height: size[1] as usize,
        depth: size[2] as usize,
    }
}

impl CommandList {
    /// Notes that a texture subresource is read or written and asks for a
    /// barrier when an earlier copy in this window conflicts.
    fn texture_hazard(&mut self, t: &Texture, mip: u32, slice: u32, write: bool) {
        let hit = self.copies.accesses.iter().any(|(u, m, s, w)| {
            u.same(t)
                && (*m == ALL || mip == ALL || *m == mip)
                && (*s == ALL || slice == ALL || *s == slice)
                && (write || *w)
        });
        if hit {
            self.order_blits();
        }
        self.copies.accesses.push((t.clone(), mip, slice, write));
    }

    /// `CopyTextureRegion` from a buffer footprint to a texture:
    /// `size` texels (a block compressed format: a multiple of the block,
    /// or up to the mip's edge).
    pub fn copy_buffer_to_texture(
        &mut self,
        dst: &TexLoc<'_>,
        src: &BufLoc<'_>,
        size: [u32; 3],
    ) -> Result<()> {
        self.live()?;
        let (_, layout, size) = check_texture(self, dst, size, "texture upload")?;
        if size.contains(&0) {
            return Ok(());
        }
        let (per_image, extent) = check_buffer(self, src, &layout, size, "texture upload")?;
        self.blit_read_hazard(src.buffer, src.offset, extent);
        self.texture_hazard(dst.texture, dst.mip, dst.slice, true);
        let (t, b) = (dst.texture.clone(), src.buffer.clone());
        let (slice, level) = tex_subresource(dst);
        let encoder = self.prepare(BLIT)?;
        // SAFETY: the texture and buffer are kept alive by this list, the
        // region and the buffer range were checked above.
        unsafe {
            if layout.option == MTLBlitOption::None {
                encoder.copyFromBuffer_sourceOffset_sourceBytesPerRow_sourceBytesPerImage_sourceSize_toTexture_destinationSlice_destinationLevel_destinationOrigin(
                    b.raw(),
                    src.offset as usize,
                    src.row_pitch as usize,
                    per_image as usize,
                    mtl_size(size),
                    t.raw(),
                    slice,
                    level,
                    mtl_origin(dst),
                );
            } else {
                encoder.copyFromBuffer_sourceOffset_sourceBytesPerRow_sourceBytesPerImage_sourceSize_toTexture_destinationSlice_destinationLevel_destinationOrigin_options(
                    b.raw(),
                    src.offset as usize,
                    src.row_pitch as usize,
                    per_image as usize,
                    mtl_size(size),
                    t.raw(),
                    slice,
                    level,
                    mtl_origin(dst),
                    layout.option,
                );
            }
        }
        self.stats.copies += 1;
        self.keep_buffers.push(b);
        self.copies.keep.push(t);
        Ok(())
    }

    /// `CopyTextureRegion` from a texture to a buffer footprint.
    pub fn copy_texture_to_buffer(
        &mut self,
        dst: &BufLoc<'_>,
        src: &TexLoc<'_>,
        size: [u32; 3],
    ) -> Result<()> {
        self.live()?;
        let (_, layout, size) = check_texture(self, src, size, "texture download")?;
        if size.contains(&0) {
            return Ok(());
        }
        let (per_image, extent) = check_buffer(self, dst, &layout, size, "texture download")?;
        self.blit_write_hazard(dst.buffer, dst.offset, extent);
        self.texture_hazard(src.texture, src.mip, src.slice, false);
        let (t, b) = (src.texture.clone(), dst.buffer.clone());
        let (slice, level) = tex_subresource(src);
        let encoder = self.prepare(BLIT)?;
        // SAFETY: as in `copy_buffer_to_texture`.
        unsafe {
            encoder.copyFromTexture_sourceSlice_sourceLevel_sourceOrigin_sourceSize_toBuffer_destinationOffset_destinationBytesPerRow_destinationBytesPerImage_options(
                t.raw(),
                slice,
                level,
                mtl_origin(src),
                mtl_size(size),
                b.raw(),
                dst.offset as usize,
                dst.row_pitch as usize,
                per_image as usize,
                layout.option,
            );
        }
        self.stats.copies += 1;
        self.keep_buffers.push(b);
        self.copies.keep.push(t);
        Ok(())
    }

    /// `CopyTextureRegion` from a texture to a texture of the same texel
    /// layout. Partial copies of depth-stencil textures are refused (Metal
    /// copies both planes or none; use [`copy_texture_resource`](Self::copy_texture_resource)).
    pub fn copy_texture(
        &mut self,
        dst: &TexLoc<'_>,
        src: &TexLoc<'_>,
        size: [u32; 3],
    ) -> Result<()> {
        self.live()?;
        let (sf, sl, size) = check_texture(self, src, size, "texture copy source")?;
        let (df, dl, _) = check_texture(self, dst, size, "texture copy destination")?;
        if size.contains(&0) {
            return Ok(());
        }
        if sf.planes() > 1 || df.planes() > 1 {
            return Err(Error::Unsupported(
                "copying one plane or a region of a depth-stencil texture".into(),
            ));
        }
        if sl.bytes != dl.bytes || sl.block != dl.block {
            return Err(Error::BadArgument(format!(
                "texture copy between {} and {}: different texel layouts",
                sf.name, df.name
            )));
        }
        if src.texture.pixel_format() != dst.texture.pixel_format() {
            return Err(Error::Unsupported(format!(
                "texture copy between {:?} and {:?} textures",
                src.texture.pixel_format(),
                dst.texture.pixel_format()
            )));
        }
        if src.texture.same(dst.texture)
            && src.mip == dst.mip
            && src.slice == dst.slice
            && (0..3).all(|a| {
                src.origin[a] < dst.origin[a] + size[a] && dst.origin[a] < src.origin[a] + size[a]
            })
        {
            return Err(Error::BadArgument("texture copy regions overlap".into()));
        }
        self.texture_hazard(src.texture, src.mip, src.slice, false);
        self.texture_hazard(dst.texture, dst.mip, dst.slice, true);
        let (s, d) = (src.texture.clone(), dst.texture.clone());
        let (ss, sm) = tex_subresource(src);
        let (ds, dm) = tex_subresource(dst);
        let encoder = self.prepare(BLIT)?;
        // SAFETY: both textures are kept alive by this list; checked above.
        unsafe {
            encoder.copyFromTexture_sourceSlice_sourceLevel_sourceOrigin_sourceSize_toTexture_destinationSlice_destinationLevel_destinationOrigin(
                s.raw(),
                ss,
                sm,
                mtl_origin(src),
                mtl_size(size),
                d.raw(),
                ds,
                dm,
                mtl_origin(dst),
            );
        }
        self.stats.copies += 1;
        self.copies.keep.push(s);
        self.copies.keep.push(d);
        Ok(())
    }

    /// `CopyResource` of two textures: every subresource. They must have the
    /// same dimension, size, mips, array size, samples and Metal format.
    pub fn copy_texture_resource(&mut self, dst: &Texture, src: &Texture) -> Result<()> {
        self.live()?;
        let (a, b) = (dst.desc(), src.desc());
        if !dst.belongs_to(self.queue().device()) || !src.belongs_to(self.queue().device()) {
            return Err(Error::BadArgument(
                "copy of a texture of another device".into(),
            ));
        }
        let same = a.dim == b.dim
            && (a.width, a.height, a.depth_or_array) == (b.width, b.height, b.depth_or_array)
            && a.mips == b.mips
            && a.samples == b.samples
            && dst.pixel_format() == src.pixel_format();
        if !same {
            return Err(Error::BadArgument(format!(
                "CopyResource between textures of different descriptions: {a:?} and {b:?}"
            )));
        }
        if dst.same(src) {
            return Err(Error::BadArgument("copy of a texture onto itself".into()));
        }
        self.texture_hazard(src, ALL, ALL, false);
        self.texture_hazard(dst, ALL, ALL, true);
        let (s, d) = (src.clone(), dst.clone());
        let encoder = self.prepare(BLIT)?;
        // SAFETY: both textures are alive (kept by this list) and match.
        unsafe { encoder.copyFromTexture_toTexture(s.raw(), d.raw()) };
        self.stats.copies += 1;
        self.copies.keep.push(s);
        self.copies.keep.push(d);
        Ok(())
    }
}

/// `(slice, level)` of a location as Metal wants them: a 3D texture has
/// slice 0 and depth in the origin.
fn tex_subresource(loc: &TexLoc<'_>) -> (usize, usize) {
    let slice = if loc.texture.desc().dim == TexDim::D3 {
        0
    } else {
        loc.slice as usize
    };
    (slice, loc.mip as usize)
}
