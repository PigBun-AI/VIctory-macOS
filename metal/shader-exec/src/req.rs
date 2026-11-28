//! The requests of `wire`, decoded. Nothing here names Metal.

use crate::wire::R;

type Res<T> = Result<T, String>;

/// One root parameter: `kind` 0 = 32-bit constants (`n32` dwords), 1 = root
/// CBV, 2 = root SRV, 3 = root UAV; `vis` is D3D12's shader visibility.
#[derive(Clone, Debug, PartialEq)]
pub struct Param {
    pub kind: u8,
    pub vis: u8,
    pub register: u32,
    pub space: u32,
    pub n32: u32,
}

/// A static sampler (the D3D12 numbers of filter, address, comparison and
/// border colour).
#[derive(Clone, Debug, PartialEq)]
pub struct StaticSampler {
    pub filter: u32,
    pub address: [u32; 3],
    pub mip_lod_bias: f32,
    pub max_anisotropy: u32,
    pub comparison: u32,
    pub border: u32,
    pub min_lod: f32,
    pub max_lod: f32,
    pub register: u32,
    pub space: u32,
    pub vis: u8,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Root {
    /// 1 = CBV/SRV/UAV heap directly indexed, 2 = sampler heap directly indexed,
    /// 4 = input layout allowed (graphics).
    pub flags: u32,
    pub params: Vec<Param>,
    pub samplers: Vec<StaticSampler>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Arg {
    Constants(Vec<u32>),
    /// A root CBV (or SRV) over a buffer of these dwords.
    View {
        srv: bool,
        data: Vec<u32>,
    },
    /// A root UAV over a zeroed buffer of this many dwords.
    Uav(u32),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Stride {
    Raw,
    /// RGBA32F elements.
    Typed,
    Structured(u32),
    /// DXGI format number, bytes per element.
    Format(u32, u32),
}

#[derive(Clone, Debug, PartialEq)]
pub enum View {
    /// RGBA32F texture SRV.
    Texture {
        w: u32,
        h: u32,
        texels: Vec<f32>,
    },
    /// RGBA32F cube SRV, texels in D3D12 subresource order (face-major).
    Cube {
        size: u32,
        mips: u32,
        texels: Vec<f32>,
    },
    RawUav(u32),
    TextureUav {
        w: u32,
        h: u32,
    },
    UintTextureUav {
        w: u32,
        h: u32,
    },
    HalfUav(u32),
    TypedUav {
        data: Vec<u32>,
        format: u32,
        elem_bytes: u32,
    },
    Cbv(Vec<u32>),
    Buffer {
        data: Vec<u32>,
        stride: Stride,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Sampler {
    pub filter: u32,
    pub address: [u32; 3],
    pub mip_lod_bias: f32,
    pub max_anisotropy: u32,
    pub comparison: u32,
    pub border: [f32; 4],
    pub min_lod: f32,
    pub max_lod: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Dispatch {
    pub roots: Vec<(u32, Arg)>,
    pub views: Vec<View>,
    pub samplers: Vec<Sampler>,
    pub groups: [u32; 3],
}

pub fn root(r: &mut R) -> Res<Root> {
    let flags = r.u32()?;
    let n = r.u32()?;
    let mut params = Vec::new();
    for _ in 0..n {
        params.push(Param {
            kind: r.u8()?,
            vis: r.u8()?,
            register: r.u32()?,
            space: r.u32()?,
            n32: r.u32()?,
        });
    }
    let n = r.u32()?;
    let mut samplers = Vec::new();
    for _ in 0..n {
        samplers.push(StaticSampler {
            filter: r.u32()?,
            address: [r.u32()?, r.u32()?, r.u32()?],
            mip_lod_bias: r.f32()?,
            max_anisotropy: r.u32()?,
            comparison: r.u32()?,
            border: r.u32()?,
            min_lod: r.f32()?,
            max_lod: r.f32()?,
            register: r.u32()?,
            space: r.u32()?,
            vis: r.u8()?,
        });
    }
    Ok(Root {
        flags,
        params,
        samplers,
    })
}

fn f32s(r: &mut R) -> Res<Vec<f32>> {
    Ok(r.u32s()?.into_iter().map(f32::from_bits).collect())
}

fn arg(r: &mut R) -> Res<Arg> {
    Ok(match r.u8()? {
        0 => Arg::Constants(r.u32s()?),
        1 => Arg::View {
            srv: r.u8()? != 0,
            data: r.u32s()?,
        },
        2 => Arg::Uav(r.u32()?),
        k => return Err(format!("root argument kind {k}")),
    })
}

fn view(r: &mut R) -> Res<View> {
    Ok(match r.u8()? {
        0 => View::Texture {
            w: r.u32()?,
            h: r.u32()?,
            texels: f32s(r)?,
        },
        1 => View::Cube {
            size: r.u32()?,
            mips: r.u32()?,
            texels: f32s(r)?,
        },
        2 => View::RawUav(r.u32()?),
        3 => View::TextureUav {
            w: r.u32()?,
            h: r.u32()?,
        },
        4 => View::UintTextureUav {
            w: r.u32()?,
            h: r.u32()?,
        },
        5 => View::HalfUav(r.u32()?),
        6 => View::TypedUav {
            data: r.u32s()?,
            format: r.u32()?,
            elem_bytes: r.u32()?,
        },
        7 => View::Cbv(r.u32s()?),
        8 => {
            let data = r.u32s()?;
            let stride = match r.u8()? {
                0 => Stride::Raw,
                1 => Stride::Typed,
                2 => Stride::Structured(r.u32()?),
                3 => Stride::Format(r.u32()?, r.u32()?),
                k => return Err(format!("buffer stride kind {k}")),
            };
            View::Buffer { data, stride }
        }
        k => return Err(format!("view kind {k}")),
    })
}

fn sampler(r: &mut R) -> Res<Sampler> {
    Ok(Sampler {
        filter: r.u32()?,
        address: [r.u32()?, r.u32()?, r.u32()?],
        mip_lod_bias: r.f32()?,
        max_anisotropy: r.u32()?,
        comparison: r.u32()?,
        border: [r.f32()?, r.f32()?, r.f32()?, r.f32()?],
        min_lod: r.f32()?,
        max_lod: r.f32()?,
    })
}

pub fn dispatch(r: &mut R) -> Res<Dispatch> {
    let n = r.u32()?;
    let mut roots = Vec::new();
    for _ in 0..n {
        let param = r.u32()?;
        roots.push((param, arg(r)?));
    }
    let n = r.u32()?;
    let mut views = Vec::new();
    for _ in 0..n {
        views.push(view(r)?);
    }
    let n = r.u32()?;
    let mut samplers = Vec::new();
    for _ in 0..n {
        samplers.push(sampler(r)?);
    }
    let groups = [r.u32()?, r.u32()?, r.u32()?];
    Ok(Dispatch {
        roots,
        views,
        samplers,
        groups,
    })
}

/// A vertex input element: the semantic, its DXGI format number and its byte offset in the
/// one vertex buffer.
#[derive(Clone, Debug, PartialEq)]
pub struct Input {
    pub semantic: String,
    pub index: u32,
    pub format: u32,
    pub offset: u32,
}

/// The pipeline half of a draw: a vertex (`mesh = false`: `stage` is the vertex shader) or a
/// mesh pipeline, its pixel shader, the render targets' DXGI formats, the depth target
/// (0 none, 1 D32_FLOAT, 2 D32_FLOAT_S8X24_UINT) and the input layout.
#[derive(Clone, Debug, PartialEq)]
pub struct Pipeline<'a> {
    pub mesh: bool,
    pub stage: &'a [u8],
    /// The geometry shader between the vertex and pixel stages (empty: none).
    pub gs: &'a [u8],
    pub ps: &'a [u8],
    pub targets: Vec<u32>,
    pub depth: u8,
    pub inputs: Vec<Input>,
}

/// The rest of a draw: one triangle (or `groups` mesh groups) into a `size` target, with the
/// vertex data and the bindings of a dispatch (`bind.groups[0]` is the mesh group count).
#[derive(Clone, Debug, PartialEq)]
pub struct Draw {
    pub vertices: Vec<u32>,
    pub stride: u32,
    pub size: [u32; 2],
    pub bind: Dispatch,
}

pub fn pipeline<'a>(r: &mut R<'a>) -> Res<Pipeline<'a>> {
    let mesh = r.u8()? != 0;
    let stage = r.bytes()?;
    let gs = r.bytes()?;
    let ps = r.bytes()?;
    let n = r.u32()?;
    let mut targets = Vec::new();
    for _ in 0..n {
        targets.push(r.u32()?);
    }
    let depth = r.u8()?;
    let n = r.u32()?;
    let mut inputs = Vec::new();
    for _ in 0..n {
        inputs.push(Input {
            semantic: String::from_utf8(r.bytes()?.to_vec()).map_err(|e| e.to_string())?,
            index: r.u32()?,
            format: r.u32()?,
            offset: r.u32()?,
        });
    }
    Ok(Pipeline {
        mesh,
        stage,
        gs,
        ps,
        targets,
        depth,
        inputs,
    })
}

pub fn draw(r: &mut R) -> Res<Draw> {
    let vertices = r.u32s()?;
    let stride = r.u32()?;
    let size = [r.u32()?, r.u32()?];
    let bind = dispatch(r)?;
    Ok(Draw {
        vertices,
        stride,
        size,
        bind,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The same bytes `victory-shader`'s `host_exec` pins as its root
    /// signature (`the_root_wire_form_is_pinned`): the two halves agree.
    #[test]
    fn the_root_wire_form_matches_the_hosts() {
        let bytes: &[u8] = &[
            3, 0, 0, 0, 2, 0, 0, 0, //
            0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 4, 0, 0, 0, //
            3, 0, 2, 0, 0, 0, 9, 0, 0, 0, 0, 0, 0, 0, //
            0, 0, 0, 0,
        ];
        let mut r = R::new(bytes);
        let root = root(&mut r).unwrap();
        r.end().unwrap();
        assert_eq!(root.flags, 3);
        assert_eq!(
            root.params,
            vec![
                Param {
                    kind: 0,
                    vis: 0,
                    register: 0,
                    space: 7,
                    n32: 4
                },
                Param {
                    kind: 3,
                    vis: 0,
                    register: 2,
                    space: 9,
                    n32: 0
                },
            ]
        );
    }

    /// The same bytes `victory-shader`'s `host_exec` pins for a pipeline
    /// (`the_pipeline_wire_form_is_pinned`).
    #[test]
    fn the_pipeline_wire_form_matches_the_hosts() {
        let bytes: &[u8] = &[
            1, // mesh
            2, 0, 0, 0, b'a', b'b', // stage
            0, 0, 0, 0, // no geometry shader
            1, 0, 0, 0, b'c', // pixel shader
            1, 0, 0, 0, 2, 0, 0, 0, // one target, R32G32B32A32_FLOAT
            0, // no depth
            1, 0, 0, 0, 1, 0, 0, 0, b'P', 0, 0, 0, 0, 6, 0, 0, 0, 0, 0, 0, 0, // POSITION0
        ];
        let mut r = R::new(bytes);
        let p = pipeline(&mut r).unwrap();
        r.end().unwrap();
        assert_eq!(
            p,
            Pipeline {
                mesh: true,
                stage: b"ab",
                gs: b"",
                ps: b"c",
                targets: vec![2],
                depth: 0,
                inputs: vec![Input {
                    semantic: "P".into(),
                    index: 0,
                    format: 6,
                    offset: 0
                }],
            }
        );
    }
}
