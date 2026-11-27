//! Just enough of the DXIL container to classify a shader before (or
//! without) handing it to Metal Shader Converter: the program header (shader
//! kind, shader model) and the `SFI0` feature mask.
//!
//! The container is DXBC-shaped: `DXBC`, a 16-byte digest, version (2 x u16),
//! total size, part count, part offsets; each part is a FourCC, a size and
//! its bytes. `DXIL` holds the program header; `SFI0` the shader feature
//! flags (a `u64`; the bit values are those of DXC's
//! `hlsl::DXIL::ShaderFeatureInfo`, which D3D12 reports as
//! `D3D_SHADER_REQUIRES_*`).

/// `D3D_SHADER_REQUIRES_*` bits of `SFI0` that matter to MSC.
pub mod feature {
    /// `D3D_SHADER_REQUIRES_DOUBLES`: double-precision arithmetic.
    pub const DOUBLES: u64 = 0x1;
    /// `D3D_SHADER_REQUIRES_11_1_DOUBLE_EXTENSIONS`: the D3D11.1 double
    /// instructions (`rcp`, `div`, `fma` on doubles; DXC sets it with
    /// `DOUBLES` for the double code it emits).
    pub const DOUBLE_EXTENSIONS_11_1: u64 = 0x20;
    /// Either double bit.
    pub const ANY_DOUBLES: u64 = DOUBLES | DOUBLE_EXTENSIONS_11_1;
    pub const STENCIL_REF: u64 = 0x200;
    pub const WAVE_OPS: u64 = 0x4000;
    pub const INT64_OPS: u64 = 0x8000;
    pub const VIEW_ID: u64 = 0x1_0000;
    pub const BARYCENTRICS: u64 = 0x2_0000;
    pub const NATIVE_LOW_PRECISION: u64 = 0x4_0000;
    pub const SHADING_RATE: u64 = 0x8_0000;
    pub const RAYTRACING_TIER_1_1: u64 = 0x10_0000;
    pub const SAMPLER_FEEDBACK: u64 = 0x20_0000;
    pub const ATOMIC_INT64_ON_TYPED_RESOURCE: u64 = 0x40_0000;
    pub const ATOMIC_INT64_ON_GROUP_SHARED: u64 = 0x80_0000;
    pub const DERIVATIVES_IN_MESH_AND_AMP: u64 = 0x100_0000;
    /// `ResourceDescriptorHeap[i]` is used.
    pub const RESOURCE_DESCRIPTOR_HEAP_INDEXING: u64 = 0x200_0000;
    /// `SamplerDescriptorHeap[i]` is used.
    pub const SAMPLER_DESCRIPTOR_HEAP_INDEXING: u64 = 0x400_0000;
    pub const ATOMIC_INT64_ON_HEAP_RESOURCE: u64 = 0x1000_0000;
}

/// The shader kind of a DXIL program header (`DXIL::ShaderKind`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    Pixel,
    Vertex,
    Geometry,
    Hull,
    Domain,
    Compute,
    Library,
    RayGeneration,
    Intersection,
    AnyHit,
    ClosestHit,
    Miss,
    Callable,
    Mesh,
    Amplification,
    Unknown(u32),
}

impl Kind {
    fn from_u32(v: u32) -> Kind {
        match v {
            0 => Kind::Pixel,
            1 => Kind::Vertex,
            2 => Kind::Geometry,
            3 => Kind::Hull,
            4 => Kind::Domain,
            5 => Kind::Compute,
            6 => Kind::Library,
            7 => Kind::RayGeneration,
            8 => Kind::Intersection,
            9 => Kind::AnyHit,
            10 => Kind::ClosestHit,
            11 => Kind::Miss,
            12 => Kind::Callable,
            13 => Kind::Mesh,
            14 => Kind::Amplification,
            x => Kind::Unknown(x),
        }
    }
}

/// What the container says about itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Info {
    /// `None` when there is no `DXIL` part.
    pub kind: Option<Kind>,
    /// Shader model (major, minor) of the program header.
    pub shader_model: Option<(u32, u32)>,
    /// `SFI0` feature mask; `None` when there is no such part.
    pub features: Option<u64>,
    /// The container carries a root signature (`RTS0`).
    pub has_root_signature: bool,
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

/// Reads the container's parts. `None` if it is not a DXBC/DXIL container.
pub fn inspect(bytes: &[u8]) -> Option<Info> {
    if bytes.get(..4)? != b"DXBC" {
        return None;
    }
    let count = u32_at(bytes, 28)? as usize;
    if count > 64 {
        return None;
    }
    let mut info = Info {
        kind: None,
        shader_model: None,
        features: None,
        has_root_signature: false,
    };
    for i in 0..count {
        let off = u32_at(bytes, 32 + 4 * i)? as usize;
        let fourcc = bytes.get(off..off + 4)?;
        let size = u32_at(bytes, off + 4)? as usize;
        let data = bytes.get(off + 8..off + 8 + size)?;
        match fourcc {
            b"DXIL" => {
                let version = u32_at(data, 0)?;
                info.kind = Some(Kind::from_u32(version >> 16));
                info.shader_model = Some(((version >> 4) & 0xf, version & 0xf));
            }
            b"SFI0" => info.features = Some(u64::from_le_bytes(data.get(..8)?.try_into().ok()?)),
            b"RTS0" => info.has_root_signature = true,
            _ => {}
        }
    }
    Some(info)
}

/// The data of the container's first part named `fourcc`.
pub fn part<'a>(bytes: &'a [u8], fourcc: &[u8; 4]) -> Option<&'a [u8]> {
    if bytes.get(..4)? != b"DXBC" {
        return None;
    }
    let count = u32_at(bytes, 28)? as usize;
    if count > 64 {
        return None;
    }
    for i in 0..count {
        let off = u32_at(bytes, 32 + 4 * i)? as usize;
        if bytes.get(off..off + 4)? == fourcc {
            let size = u32_at(bytes, off + 4)? as usize;
            return bytes.get(off + 8..off + 8 + size);
        }
    }
    None
}

/// Whether the container holds DXBC bytecode (a `SHDR` or `SHEX` part and no
/// `DXIL` part), that is the output of the old compiler, which MSC cannot
/// read: it has to go through the DXBC converter first.
pub fn is_dxbc(bytes: &[u8]) -> bool {
    let Some(info) = inspect(bytes) else {
        return false;
    };
    if info.kind.is_some() {
        return false;
    }
    let count = u32_at(bytes, 28).unwrap_or(0) as usize;
    (0..count.min(64)).any(|i| {
        u32_at(bytes, 32 + 4 * i)
            .and_then(|off| bytes.get(off as usize..off as usize + 4))
            .is_some_and(|f| f == b"SHDR" || f == b"SHEX")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A synthetic container with the three parts the reader looks at.
    fn container(version: u32, flags: u64, rts0: bool) -> Vec<u8> {
        let parts: Vec<([u8; 4], Vec<u8>)> = {
            let mut p = vec![
                (*b"SFI0", flags.to_le_bytes().to_vec()),
                (*b"DXIL", version.to_le_bytes().to_vec()),
            ];
            if rts0 {
                p.push((*b"RTS0", vec![0; 8]));
            }
            p
        };
        let header = 32 + 4 * parts.len();
        let mut out = b"DXBC".to_vec();
        out.extend([0u8; 16]);
        out.extend(1u16.to_le_bytes());
        out.extend(0u16.to_le_bytes());
        out.extend(0u32.to_le_bytes());
        out.extend((parts.len() as u32).to_le_bytes());
        let mut off = header;
        for (_, d) in &parts {
            out.extend((off as u32).to_le_bytes());
            off += 8 + d.len();
        }
        for (f, d) in &parts {
            out.extend(f);
            out.extend((d.len() as u32).to_le_bytes());
            out.extend(d);
        }
        out
    }

    #[test]
    fn reads_kind_model_and_features() {
        // Compute (5), shader model 6.6: 0x0005_0066.
        let c = container(
            0x0005_0066,
            feature::RESOURCE_DESCRIPTOR_HEAP_INDEXING,
            false,
        );
        let i = inspect(&c).unwrap();
        assert_eq!(i.kind, Some(Kind::Compute));
        assert_eq!(i.shader_model, Some((6, 6)));
        assert_eq!(i.features, Some(feature::RESOURCE_DESCRIPTOR_HEAP_INDEXING));
        assert!(!i.has_root_signature);
        assert!(
            inspect(&container(0x000D_0065, 0, true))
                .unwrap()
                .has_root_signature
        );
        assert_eq!(
            inspect(&container(0x000D_0065, 0, false)).unwrap().kind,
            Some(Kind::Mesh)
        );
    }

    #[test]
    fn refuses_what_is_not_a_container() {
        assert!(inspect(b"").is_none());
        assert!(inspect(b"NOPE0000000000000000000000000000000000").is_none());
        let mut c = container(0x0005_0066, 0, false);
        c.truncate(40);
        assert!(inspect(&c).is_none());
    }
}
