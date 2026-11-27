//! Sampler states for descriptor tables.
//!
//! A shader converted by Metal Shader Converter reads a sampler from a
//! descriptor-table entry holding the state's `gpuResourceID`
//! ([`IrDescriptorTableEntry::sampler`]). The state must be made with
//! `supportArgumentBuffers`. Plain Rust in, so this module is as free of the
//! wire protocol as the rest of the core.

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_metal::{
    MTLCompareFunction, MTLDevice, MTLSamplerAddressMode, MTLSamplerBorderColor,
    MTLSamplerDescriptor, MTLSamplerMinMagFilter, MTLSamplerMipFilter, MTLSamplerState,
};

use crate::descriptors::IrDescriptorTableEntry;
use crate::device::Device;
use crate::error::{Error, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Filter {
    #[default]
    Nearest,
    Linear,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Address {
    #[default]
    Repeat,
    MirrorRepeat,
    ClampToEdge,
    ClampToBorder,
    MirrorClampToEdge,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Border {
    #[default]
    TransparentBlack,
    OpaqueBlack,
    OpaqueWhite,
}

/// The compare functions in Metal's order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Compare {
    Never,
    Less,
    Equal,
    LessEqual,
    Greater,
    NotEqual,
    GreaterEqual,
    Always,
}

/// What a sampler state is made from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SamplerSpec {
    pub min: Filter,
    pub mag: Filter,
    /// `None`: no mip filtering (level 0 only).
    pub mip: Option<Filter>,
    pub address: [Address; 3],
    pub border: Border,
    /// `Some`: a comparison sampler.
    pub compare: Option<Compare>,
    pub min_lod: f32,
    pub max_lod: f32,
    /// 1 = off.
    pub max_anisotropy: u32,
    /// Goes into the table entry, not the state.
    pub lod_bias: f32,
}

impl Default for SamplerSpec {
    fn default() -> Self {
        SamplerSpec {
            min: Filter::Nearest,
            mag: Filter::Nearest,
            mip: Some(Filter::Nearest),
            address: [Address::Repeat; 3],
            border: Border::TransparentBlack,
            compare: None,
            min_lod: 0.0,
            max_lod: 1000.0,
            max_anisotropy: 1,
            lod_bias: 0.0,
        }
    }
}

/// A sampler state (kept alive by this handle).
#[derive(Clone)]
pub struct Sampler {
    state: Retained<ProtocolObject<dyn MTLSamplerState>>,
    lod_bias: f32,
}

// Metal sampler states are immutable and thread safe.
unsafe impl Send for Sampler {}
unsafe impl Sync for Sampler {}

fn min_mag(f: Filter) -> MTLSamplerMinMagFilter {
    match f {
        Filter::Nearest => MTLSamplerMinMagFilter::Nearest,
        Filter::Linear => MTLSamplerMinMagFilter::Linear,
    }
}

fn addr(a: Address) -> MTLSamplerAddressMode {
    match a {
        Address::Repeat => MTLSamplerAddressMode::Repeat,
        Address::MirrorRepeat => MTLSamplerAddressMode::MirrorRepeat,
        Address::ClampToEdge => MTLSamplerAddressMode::ClampToEdge,
        Address::ClampToBorder => MTLSamplerAddressMode::ClampToBorderColor,
        Address::MirrorClampToEdge => MTLSamplerAddressMode::MirrorClampToEdge,
    }
}

fn cmp(c: Compare) -> MTLCompareFunction {
    match c {
        Compare::Never => MTLCompareFunction::Never,
        Compare::Less => MTLCompareFunction::Less,
        Compare::Equal => MTLCompareFunction::Equal,
        Compare::LessEqual => MTLCompareFunction::LessEqual,
        Compare::Greater => MTLCompareFunction::Greater,
        Compare::NotEqual => MTLCompareFunction::NotEqual,
        Compare::GreaterEqual => MTLCompareFunction::GreaterEqual,
        Compare::Always => MTLCompareFunction::Always,
    }
}

impl Device {
    /// Makes a sampler state usable from descriptor tables.
    pub fn create_sampler(&self, spec: &SamplerSpec) -> Result<Sampler> {
        let d = MTLSamplerDescriptor::new();
        d.setMinFilter(min_mag(spec.min));
        d.setMagFilter(min_mag(spec.mag));
        d.setMipFilter(match spec.mip {
            None => MTLSamplerMipFilter::NotMipmapped,
            Some(Filter::Nearest) => MTLSamplerMipFilter::Nearest,
            Some(Filter::Linear) => MTLSamplerMipFilter::Linear,
        });
        d.setSAddressMode(addr(spec.address[0]));
        d.setTAddressMode(addr(spec.address[1]));
        d.setRAddressMode(addr(spec.address[2]));
        d.setBorderColor(match spec.border {
            Border::TransparentBlack => MTLSamplerBorderColor::TransparentBlack,
            Border::OpaqueBlack => MTLSamplerBorderColor::OpaqueBlack,
            Border::OpaqueWhite => MTLSamplerBorderColor::OpaqueWhite,
        });
        if let Some(c) = spec.compare {
            d.setCompareFunction(cmp(c));
        }
        d.setLodMinClamp(spec.min_lod);
        d.setLodMaxClamp(spec.max_lod);
        d.setMaxAnisotropy(spec.max_anisotropy.clamp(1, 16) as usize);
        d.setSupportArgumentBuffers(true);
        let state = self
            .raw()
            .newSamplerStateWithDescriptor(&d)
            .ok_or_else(|| Error::Metal("newSamplerStateWithDescriptor returned nil".into()))?;
        Ok(Sampler {
            state,
            lod_bias: spec.lod_bias,
        })
    }
}

impl Sampler {
    /// `gpuResourceID` of the state.
    pub fn resource_id(&self) -> u64 {
        let id = self.state.gpuResourceID();
        // `MTLResourceID` is `struct { uint64_t _impl; }` (Metal's header);
        // objc2 keeps the field private.
        const _: () = assert!(std::mem::size_of::<objc2_metal::MTLResourceID>() == 8);
        // SAFETY: same size (asserted), plain integer inside.
        unsafe { std::mem::transmute::<objc2_metal::MTLResourceID, u64>(id) }
    }

    /// The descriptor-table entry that makes a shader use this sampler.
    pub fn entry(&self) -> IrDescriptorTableEntry {
        IrDescriptorTableEntry::sampler(self.resource_id(), self.lod_bias)
    }
}
