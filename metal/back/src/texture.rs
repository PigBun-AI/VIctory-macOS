//! Textures: private-storage `MTLTexture`s, their views, and what a shader
//! needs to find them.
//!
//! A texture is made from a [`TextureDesc`] (dimension, DXGI format number,
//! size, mips, samples, usage bits; see [`crate::format`] for the format
//! rules) either committed ([`Device::create_texture`], its own memory) or
//! placed in a [`Heap`](crate::heap::Heap). It is untracked, in the `Resource`
//! residency set, and retired when the last handle drops, after the GPU work
//! that may still use it (`crate::residency`).
//!
//! # Descriptors and views
//!
//! A shader reaches a texture through a descriptor-table entry holding the
//! texture's `gpuResourceID` (`IRDescriptorTableSetTexture`, checked
//! 2026-11-27 against MSC 4.0.3: `gpuVA = 0`, `textureViewID` = the id,
//! `metadata` low 32 bits = the minimum LOD clamp). The id must name an
//! `MTLTexture` of the type, format and range the shader declares:
//! [`Texture::view`] makes one with
//! `newTextureViewWithPixelFormat:textureType:levels:slices:swizzle:` and
//! returns its id. A view equal to the whole texture (own format, own type,
//! every mip and slice, no swizzle) is the texture itself, so the common SRV
//! costs nothing. Views are cached per texture and retired with it, like the
//! texture buffers of typed buffer views.
//!
//! Limits of views that Metal has and the original API does not:
//!
//! * a view of a 3D texture is the whole depth (a UAV over a range of W
//!   slices is refused with `Unsupported`);
//! * a cube view takes 6 slices (cube array: a multiple of 6) of a 2D array.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_foundation::{NSRange, NSString};
use objc2_metal::{
    MTLAllocation, MTLCPUCacheMode, MTLDevice, MTLHazardTrackingMode, MTLPixelFormat, MTLResource,
    MTLStorageMode, MTLTexture, MTLTextureDescriptor, MTLTextureSwizzle, MTLTextureSwizzleChannels,
    MTLTextureType,
};

use crate::device::{Device, DeviceInner};
use crate::error::{Error, Result};
use crate::format;
use crate::heap::Heap;
use crate::residency::{Retired, SetKind};
use crate::typed::resource_id_of;

/// Dimension of a texture resource.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TexDim {
    /// 1D (or an array of 1D textures).
    D1,
    /// 2D (or an array; a cube map is an array of 6 and a cube view).
    D2,
    /// 3D (volume).
    D3,
}

/// What a texture is: the arguments of `CreateTexture`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TextureDesc {
    pub dim: TexDim,
    /// DXGI format number ([`crate::format`]).
    pub format: u32,
    pub width: u32,
    /// 1 for 1D.
    pub height: u32,
    /// Depth of a 3D texture, array size of the others.
    pub depth_or_array: u32,
    /// Mip levels, at least 1.
    pub mips: u32,
    /// Samples per texel: 1, 2, 4 or 8.
    pub samples: u32,
    /// `tex_usage` bits of the proto (SRV 1, UAV 2, render target 4,
    /// depth-stencil 8).
    pub usage: u32,
}

impl TextureDesc {
    /// A 2D texture of one slice and one mip.
    pub fn tex2d(format: u32, width: u32, height: u32, usage: u32) -> Self {
        TextureDesc {
            dim: TexDim::D2,
            format,
            width,
            height,
            depth_or_array: 1,
            mips: 1,
            samples: 1,
            usage,
        }
    }

    /// Array slices (1 for a 3D texture).
    pub fn array_len(&self) -> u32 {
        match self.dim {
            TexDim::D3 => 1,
            _ => self.depth_or_array,
        }
    }

    /// Depth of mip 0 (1 unless 3D).
    pub fn depth(&self) -> u32 {
        match self.dim {
            TexDim::D3 => self.depth_or_array,
            _ => 1,
        }
    }

    /// Size in texels of mip `mip`: `[width, height, depth]`, each at least 1.
    pub fn mip_size(&self, mip: u32) -> [u32; 3] {
        let s = |n: u32| (n >> mip).max(1);
        [s(self.width), s(self.height), s(self.depth())]
    }

    /// The most mip levels a texture of this size can have.
    pub fn max_mips(&self) -> u32 {
        let m = self.width.max(self.height).max(self.depth());
        32 - m.leading_zeros()
    }

    /// `subresource count` of the original API: mips x array x planes.
    pub fn subresources(&self) -> Result<u32> {
        let f = format::require(self.format)?;
        Ok(self.mips * self.array_len() * f.planes())
    }

    fn texture_type(&self) -> MTLTextureType {
        let array = self.array_len() > 1;
        match (self.dim, self.samples > 1, array) {
            (TexDim::D1, _, false) => MTLTextureType::Type1D,
            (TexDim::D1, _, true) => MTLTextureType::Type1DArray,
            (TexDim::D2, false, false) => MTLTextureType::Type2D,
            (TexDim::D2, false, true) => MTLTextureType::Type2DArray,
            (TexDim::D2, true, false) => MTLTextureType::Type2DMultisample,
            (TexDim::D2, true, true) => MTLTextureType::Type2DMultisampleArray,
            (TexDim::D3, ..) => MTLTextureType::Type3D,
        }
    }

    /// Checks everything Metal would refuse with a nil and no reason.
    pub fn validate(&self) -> Result<()> {
        let bad = |m: String| Err(Error::BadArgument(format!("texture {self:?}: {m}")));
        let f = format::require(self.format)?;
        if self.width == 0 || self.height == 0 || self.depth_or_array == 0 {
            return bad("a size is 0".into());
        }
        if self.dim == TexDim::D1 && self.height != 1 {
            return bad("a 1D texture has height 1".into());
        }
        if self.mips == 0 || self.mips > self.max_mips() {
            return bad(format!("{} mips (at most {})", self.mips, self.max_mips()));
        }
        if !matches!(self.samples, 1 | 2 | 4 | 8) {
            return bad(format!("{} samples", self.samples));
        }
        if self.samples > 1 {
            if self.dim != TexDim::D2 || self.mips != 1 {
                return bad("multisampling needs a 2D texture of one mip".into());
            }
            if f.is_compressed() {
                return bad("multisampling a compressed format".into());
            }
        }
        if self.usage & !0xF != 0 {
            return bad("unknown usage bits".into());
        }
        if self.usage & 0b1100 != 0 && self.dim == TexDim::D3 && f.has_depth_stencil() {
            return bad("a 3D depth target".into());
        }
        Ok(())
    }

    /// The `MTLTextureDescriptor` of the texture: private storage, untracked
    /// (the backend orders everything with barriers), usage from the bits.
    pub(crate) fn metal_descriptor(&self) -> Result<Retained<MTLTextureDescriptor>> {
        self.validate()?;
        let pixel = format::texture_pixel_format(self.format, self.usage)?;
        let usage = format::texture_usage(self.format, self.usage)?;
        let d = MTLTextureDescriptor::new();
        d.setTextureType(self.texture_type());
        d.setPixelFormat(pixel);
        // SAFETY: plain value setters; `validate` checked the ranges.
        unsafe {
            d.setWidth(self.width as usize);
            d.setHeight(self.height as usize);
            d.setDepth(self.depth() as usize);
            d.setMipmapLevelCount(self.mips as usize);
            d.setSampleCount(self.samples as usize);
            d.setArrayLength(self.array_len() as usize);
        }
        d.setStorageMode(MTLStorageMode::Private);
        d.setCpuCacheMode(MTLCPUCacheMode::DefaultCache);
        d.setHazardTrackingMode(MTLHazardTrackingMode::Untracked);
        d.setUsage(usage);
        Ok(d)
    }
}

/// How a shader sees a texture view (the shader's texture type).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ViewDim {
    D1,
    D1Array,
    D2,
    D2Array,
    D2Ms,
    D2MsArray,
    D3,
    /// Six slices of a 2D array, from `slice_first`.
    Cube,
    /// A multiple of six slices.
    CubeArray,
}

impl ViewDim {
    fn texture_type(self) -> MTLTextureType {
        match self {
            ViewDim::D1 => MTLTextureType::Type1D,
            ViewDim::D1Array => MTLTextureType::Type1DArray,
            ViewDim::D2 => MTLTextureType::Type2D,
            ViewDim::D2Array => MTLTextureType::Type2DArray,
            ViewDim::D2Ms => MTLTextureType::Type2DMultisample,
            ViewDim::D2MsArray => MTLTextureType::Type2DMultisampleArray,
            ViewDim::D3 => MTLTextureType::Type3D,
            ViewDim::Cube => MTLTextureType::TypeCube,
            ViewDim::CubeArray => MTLTextureType::TypeCubeArray,
        }
    }
}

/// Component sources of a swizzle: 0-3 = R, G, B, A, 4 = zero, 5 = one.
pub const SWIZZLE_IDENTITY: [u8; 4] = [0, 1, 2, 3];

/// Which part of a texture a descriptor shows and how.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ViewSpec {
    /// DXGI number of the view's format; 0 = the texture's own.
    pub format: u32,
    pub dim: ViewDim,
    pub mip_first: u32,
    /// 0 = every mip from `mip_first`.
    pub mip_count: u32,
    pub slice_first: u32,
    /// 0 = every slice from `slice_first`.
    pub slice_count: u32,
    /// 0 depth or colour, 1 stencil.
    pub plane: u32,
    /// Component sources, [`SWIZZLE_IDENTITY`] for none.
    pub swizzle: [u8; 4],
}

impl ViewSpec {
    /// A view of everything in the texture's own format, with `dim`.
    pub fn whole(dim: ViewDim) -> Self {
        ViewSpec {
            format: 0,
            dim,
            mip_first: 0,
            mip_count: 0,
            slice_first: 0,
            slice_count: 0,
            plane: 0,
            swizzle: SWIZZLE_IDENTITY,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct ViewKey {
    pixel: usize,
    ty: usize,
    mips: (u32, u32),
    slices: (u32, u32),
    swizzle: [u8; 4],
}

struct ViewObject {
    texture: Retained<ProtocolObject<dyn MTLTexture>>,
    resource_id: u64,
}

pub(crate) struct TextureInner {
    raw: Retained<ProtocolObject<dyn MTLTexture>>,
    device: Arc<DeviceInner>,
    desc: TextureDesc,
    pixel: MTLPixelFormat,
    resource_id: u64,
    _heap: Option<Heap>,
    views: Mutex<HashMap<ViewKey, ViewObject>>,
}

// A Metal texture is thread-safe.
unsafe impl Send for TextureInner {}
unsafe impl Sync for TextureInner {}

impl Drop for TextureInner {
    fn drop(&mut self) {
        let waits = self.device.in_flight_waits();
        let views = std::mem::take(self.views.get_mut().unwrap_or_else(|e| e.into_inner()));
        for (_, v) in views {
            self.device.residency.retire(Retired {
                object: ProtocolObject::from_retained(v.texture),
                kind: SetKind::Resource,
                waits: waits.clone(),
                keep: None,
            });
        }
        self.device.residency.retire(Retired {
            object: ProtocolObject::from_retained(self.raw.clone()),
            kind: SetKind::Resource,
            waits,
            keep: None,
        });
    }
}

/// A texture. Cloning shares it; the last drop retires it (deferred).
#[derive(Clone)]
pub struct Texture {
    pub(crate) inner: Arc<TextureInner>,
}

impl std::fmt::Debug for Texture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Texture")
            .field("desc", &self.inner.desc)
            .field("pixel", &self.inner.pixel)
            .finish()
    }
}

impl Device {
    /// A texture with its own memory.
    pub fn create_texture(&self, desc: &TextureDesc) -> Result<Texture> {
        let d = desc.metal_descriptor()?;
        let raw = self
            .inner
            .raw
            .newTextureWithDescriptor(&d)
            .ok_or_else(|| Error::Metal(format!("newTexture({desc:?}) returned nil")))?;
        Texture::finish(self, raw, *desc, None)
    }
}

impl Texture {
    pub(crate) fn finish(
        device: &Device,
        raw: Retained<ProtocolObject<dyn MTLTexture>>,
        desc: TextureDesc,
        heap: Option<Heap>,
    ) -> Result<Texture> {
        raw.setLabel(Some(&NSString::from_str("victory texture")));
        let allocation: &ProtocolObject<dyn MTLAllocation> = ProtocolObject::from_ref(&*raw);
        device.inner.residency.add(SetKind::Resource, allocation);
        let pixel = raw.pixelFormat();
        let resource_id = resource_id_of(raw.gpuResourceID());
        Ok(Texture {
            inner: Arc::new(TextureInner {
                raw,
                device: device.inner.clone(),
                desc,
                pixel,
                resource_id,
                _heap: heap,
                views: Mutex::new(HashMap::new()),
            }),
        })
    }

    /// What it was made as.
    pub fn desc(&self) -> &TextureDesc {
        &self.inner.desc
    }

    /// The Metal format it was made with.
    pub fn pixel_format(&self) -> MTLPixelFormat {
        self.inner.pixel
    }

    /// `MTLTexture.allocatedSize`.
    pub fn alloc_size(&self) -> u64 {
        MTLResource::allocatedSize(&*self.inner.raw) as u64
    }

    /// The `gpuResourceID` of the texture itself (the whole-texture view).
    pub fn resource_id(&self) -> u64 {
        self.inner.resource_id
    }

    pub(crate) fn raw(&self) -> &ProtocolObject<dyn MTLTexture> {
        &self.inner.raw
    }

    pub(crate) fn same(&self, other: &Texture) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }

    pub(crate) fn belongs_to(&self, device: &Device) -> bool {
        Arc::ptr_eq(&self.inner.device, &device.inner)
    }

    /// How many view objects the texture holds (for tests).
    pub fn view_count(&self) -> usize {
        self.inner
            .views
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .len()
    }

    /// Whether this texture is in a residency set.
    pub fn is_resident(&self) -> bool {
        let a: &ProtocolObject<dyn MTLAllocation> = ProtocolObject::from_ref(&*self.inner.raw);
        self.inner.device.residency.contains(SetKind::Resource, a)
    }

    /// The `gpuResourceID` a descriptor names for `spec`. The same request
    /// gives the same id; a view equal to the whole texture is the texture.
    pub fn view(&self, spec: &ViewSpec) -> Result<u64> {
        self.view_texture(spec).map(|(_, id)| id)
    }

    /// As [`view`](Self::view), with the view's Metal texture too (what a
    /// render pass attaches).
    pub(crate) fn view_texture(
        &self,
        spec: &ViewSpec,
    ) -> Result<(Retained<ProtocolObject<dyn MTLTexture>>, u64)> {
        let d = &self.inner.desc;
        let bad = |m: String| Err(Error::BadArgument(format!("view {spec:?} of {d:?}: {m}")));
        let f = format::require(d.format)?;
        // Which Metal format the view has.
        let pixel = if spec.plane == 1 {
            if !f.has_depth_stencil() {
                return bad("plane 1 of a texture without stencil".into());
            }
            MTLPixelFormat::X32_Stencil8
        } else {
            format::view_pixel_format(d.format, spec.format, d.usage)?
        };
        // Mips.
        if spec.mip_first >= d.mips {
            return bad(format!("first mip {} of {}", spec.mip_first, d.mips));
        }
        let mip_count = if spec.mip_count == 0 {
            d.mips - spec.mip_first
        } else {
            spec.mip_count
        };
        if spec.mip_first + mip_count > d.mips {
            return bad("mips run past the texture".into());
        }
        // Slices.
        let total = d.array_len();
        let (slice_first, slice_count) = match (d.dim, spec.dim) {
            (TexDim::D3, ViewDim::D3) => {
                if spec.slice_first != 0 {
                    return Err(Error::Unsupported(format!(
                        "a view of a range of W slices of a 3D texture ({spec:?})"
                    )));
                }
                (0, 1)
            }
            (TexDim::D3, _) | (_, ViewDim::D3) => {
                return bad("a 3D view needs a 3D texture and the other way round".into());
            }
            (TexDim::D1, ViewDim::D1 | ViewDim::D1Array)
            | (TexDim::D2, ViewDim::D2 | ViewDim::D2Array | ViewDim::Cube | ViewDim::CubeArray) => {
                self.slice_range(spec, total)?
            }
            (TexDim::D2, ViewDim::D2Ms | ViewDim::D2MsArray) if d.samples > 1 => {
                self.slice_range(spec, total)?
            }
            _ => return bad("view dimension does not fit the texture".into()),
        };
        if (d.samples > 1) != matches!(spec.dim, ViewDim::D2Ms | ViewDim::D2MsArray) {
            return bad("multisampled and single-sample views do not mix".into());
        }
        match spec.dim {
            ViewDim::Cube if slice_count != 6 => return bad("a cube view has 6 faces".into()),
            ViewDim::CubeArray if slice_count % 6 != 0 || slice_count == 0 => {
                return bad("a cube array view takes a multiple of 6 faces".into());
            }
            ViewDim::D1 | ViewDim::D2 | ViewDim::D2Ms if slice_count != 1 => {
                return bad("a non-array view takes one slice".into());
            }
            _ => {}
        }
        let ty = spec.dim.texture_type();
        let natural = pixel == self.inner.pixel
            && ty == d.texture_type()
            && mip_count == d.mips
            && spec.mip_first == 0
            && slice_first == 0
            && slice_count == if d.dim == TexDim::D3 { 1 } else { total }
            && spec.swizzle == SWIZZLE_IDENTITY;
        if natural {
            return Ok((self.inner.raw.clone(), self.inner.resource_id));
        }
        let key = ViewKey {
            pixel: pixel.0,
            ty: ty.0,
            mips: (spec.mip_first, mip_count),
            slices: (slice_first, slice_count),
            swizzle: spec.swizzle,
        };
        let mut views = self.inner.views.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(v) = views.get(&key) {
            return Ok((v.texture.clone(), v.resource_id));
        }
        let swizzle = swizzle_channels(spec.swizzle)?;
        // SAFETY: the ranges were checked against the texture above.
        let view = unsafe {
            self.inner
                .raw
                .newTextureViewWithPixelFormat_textureType_levels_slices_swizzle(
                    pixel,
                    ty,
                    NSRange::new(spec.mip_first as usize, mip_count as usize),
                    NSRange::new(slice_first as usize, slice_count as usize),
                    swizzle,
                )
        }
        .ok_or_else(|| {
            Error::Metal(format!(
                "newTextureView({pixel:?}, {ty:?}) of {:?} returned nil",
                self.inner.pixel
            ))
        })?;
        view.setLabel(Some(&NSString::from_str("victory texture view")));
        let allocation: &ProtocolObject<dyn MTLAllocation> = ProtocolObject::from_ref(&*view);
        self.inner
            .device
            .residency
            .add(SetKind::Resource, allocation);
        let resource_id = resource_id_of(view.gpuResourceID());
        views.insert(
            key,
            ViewObject {
                texture: view.clone(),
                resource_id,
            },
        );
        Ok((view, resource_id))
    }

    fn slice_range(&self, spec: &ViewSpec, total: u32) -> Result<(u32, u32)> {
        let count = if spec.slice_count == 0 {
            total.saturating_sub(spec.slice_first)
        } else {
            spec.slice_count
        };
        if spec.slice_first >= total || spec.slice_first + count > total || count == 0 {
            return Err(Error::BadArgument(format!(
                "slices {}+{} of a texture with {total}",
                spec.slice_first, count
            )));
        }
        Ok((spec.slice_first, count))
    }
}

fn swizzle_channels(s: [u8; 4]) -> Result<MTLTextureSwizzleChannels> {
    let one = |n: u8| -> Result<MTLTextureSwizzle> {
        Ok(match n {
            0 => MTLTextureSwizzle::Red,
            1 => MTLTextureSwizzle::Green,
            2 => MTLTextureSwizzle::Blue,
            3 => MTLTextureSwizzle::Alpha,
            4 => MTLTextureSwizzle::Zero,
            5 => MTLTextureSwizzle::One,
            _ => return Err(Error::BadArgument(format!("swizzle source {n}"))),
        })
    };
    Ok(MTLTextureSwizzleChannels {
        red: one(s[0])?,
        green: one(s[1])?,
        blue: one(s[2])?,
        alpha: one(s[3])?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mip_sizes_and_limits() {
        let d = TextureDesc {
            dim: TexDim::D2,
            format: format::dxgi::R8G8B8A8_UNORM,
            width: 100,
            height: 30,
            depth_or_array: 6,
            mips: 7,
            samples: 1,
            usage: 1,
        };
        assert_eq!(d.max_mips(), 7);
        assert!(d.validate().is_ok());
        assert_eq!(d.mip_size(0), [100, 30, 1]);
        assert_eq!(d.mip_size(6), [1, 1, 1]);
        assert_eq!(d.mip_size(2), [25, 7, 1]);
        assert_eq!(d.subresources().unwrap(), 42);
        let mut e = d;
        e.mips = 8;
        assert!(e.validate().is_err());
        e = d;
        e.samples = 3;
        assert!(e.validate().is_err());
        e = d;
        e.samples = 4;
        assert!(e.validate().is_err(), "multisample with mips");
        e.mips = 1;
        assert!(e.validate().is_ok());
        let one_d = TextureDesc {
            dim: TexDim::D1,
            height: 2,
            ..d
        };
        assert!(one_d.validate().is_err());
    }

    #[test]
    fn depth_stencil_has_two_planes() {
        let d = TextureDesc::tex2d(format::dxgi::D24_UNORM_S8_UINT, 8, 8, 8);
        assert_eq!(d.subresources().unwrap(), 2);
    }
}
