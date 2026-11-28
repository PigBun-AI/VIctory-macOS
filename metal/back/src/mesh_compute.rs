//! Mesh shader as a compute pass (docs/metal/mesh-compute.md): the scratch
//! layout a converted mesh shader writes and the draw that reads it.
//!
//! PROTOTYPE. A mesh threadgroup that Metal cannot take (output over 32 KB,
//! or one Apple's GPU compiler crashes on) runs as a compute threadgroup of the
//! same size: the group's `SetMeshOutputCounts`, `EmitIndices` and output
//! stores become stores into one raw buffer, the *scratch*, and a vertex
//! shader that fetches from the scratch draws the triangles. This file holds
//! what both sides agree on, the [`Layout`], which the lowering (the
//! translator, `tools/metal/ms2cs.py` in the prototype) writes as a text
//! file (`vmcl 1`, one line per field) and the runtime reads.
//!
//! ```text
//! group g, at g * group_stride
//!     0                 u32 vertex count, u32 primitive count
//!     vertex_head + v * vertex_stride        vertex v: its elements
//!     prim_area   + p * prim_stride          primitive p: u32 indices
//!                                            (v0 | v1 << 8 | v2 << 16), its elements
//! ```
//!
//! Every component is 4 bytes (a 16-bit output is widened; bool is a `u32`).
//! A group needs no clearing: counts say what is live, and a primitive the
//! shader culled (zero indices from `emit_indices`, or `SV_CullPrimitive`) is
//! skipped by the vertex shader.

use std::fmt;

/// Which scratch region an element belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Region {
    /// A per-vertex output.
    Vertex,
    /// A per-primitive output.
    Primitive,
}

/// What an element's components are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scalar {
    Float,
    Uint,
    Int,
    /// `SV_CullPrimitive`'s: stored as a `u32`.
    Bool,
}

/// `DXIL::SemanticKind` values the layout acts on.
pub mod kind {
    /// A plain user semantic.
    pub const ARBITRARY: u32 = 0;
    /// `SV_Position`.
    pub const POSITION: u32 = 3;
    /// `SV_CullPrimitive`.
    pub const CULL_PRIMITIVE: u32 = 30;
}

/// One output element.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Element {
    pub region: Region,
    /// Semantic name as the signature spells it (`TEXCOORD`).
    pub name: String,
    /// Semantic index.
    pub index: u32,
    /// `DXIL::SemanticKind`.
    pub kind: u32,
    pub scalar: Scalar,
    pub rows: u32,
    pub cols: u32,
    /// Byte offset inside the vertex or primitive record.
    pub offset: u32,
    /// `DXIL::InterpolationMode` (1 constant, 2 linear, 3 centroid,
    /// 4 noperspective, ...).
    pub interpolation: u32,
}

impl Element {
    /// Bytes the element takes in the scratch.
    pub fn bytes(&self) -> u32 {
        4 * self.rows * self.cols
    }
}

/// Why a layout file was not understood.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutError(pub String);

impl fmt::Display for LayoutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "mesh scratch layout: {}", self.0)
    }
}

impl std::error::Error for LayoutError {}

/// The scratch layout of one converted mesh shader.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    /// Threads of the mesh threadgroup, the compute shader's too.
    pub threads: [u32; 3],
    /// Declared maximum vertices and primitives.
    pub max_vertices: u32,
    pub max_primitives: u32,
    pub vertex_stride: u32,
    pub prim_stride: u32,
    /// Byte offset of vertex 0 and of primitive 0 in a group.
    pub vertex_head: u32,
    pub prim_area: u32,
    /// Bytes of one group.
    pub group_stride: u32,
    /// Register space and register of the scratch UAV the compute shader
    /// declares (and of the SRV the vertex shader declares).
    pub space: u32,
    pub register: u32,
    /// The resource kinds the compute form may read through heap-indexed or
    /// table views, as a bit set of `DXIL::ResourceKind` (all bits when the
    /// text has no `reads` line): what the pass-split rule matches.
    pub reads: u32,
    pub elements: Vec<Element>,
}

impl Layout {
    /// Reads the text the lowering wrote.
    pub fn parse(text: &str) -> Result<Layout, LayoutError> {
        let bad = |m: String| Err(LayoutError(m));
        let mut lines = text.lines();
        if lines.next().map(str::trim) != Some("vmcl 1") {
            return bad("not a `vmcl 1` file".into());
        }
        let mut l = Layout {
            threads: [0; 3],
            max_vertices: 0,
            max_primitives: 0,
            vertex_stride: 0,
            prim_stride: 0,
            vertex_head: 0,
            prim_area: 0,
            group_stride: 0,
            space: 0,
            register: 0,
            reads: u32::MAX,
            elements: Vec::new(),
        };
        for line in lines {
            let w: Vec<&str> = line.split_whitespace().collect();
            let Some((&key, args)) = w.split_first() else {
                continue;
            };
            let num = |i: usize| -> Result<u32, LayoutError> {
                args.get(i)
                    .and_then(|a| a.parse().ok())
                    .ok_or_else(|| LayoutError(format!("`{line}`: field {i} is no number")))
            };
            match key {
                "threads" => l.threads = [num(0)?, num(1)?, num(2)?],
                "maxv" => l.max_vertices = num(0)?,
                "maxp" => l.max_primitives = num(0)?,
                "vstride" => l.vertex_stride = num(0)?,
                "pstride" => l.prim_stride = num(0)?,
                "vhead" => l.vertex_head = num(0)?,
                "parea" => l.prim_area = num(0)?,
                "gstride" => l.group_stride = num(0)?,
                "space" => l.space = num(0)?,
                "register" => l.register = num(0)?,
                "reads" => {
                    l.reads = args
                        .first()
                        .and_then(|a| u32::from_str_radix(a.trim_start_matches("0x"), 16).ok())
                        .ok_or_else(|| LayoutError(format!("`{line}`: no hexadecimal mask")))?
                }
                "v" | "p" => {
                    // v|p NAME INDEX KIND TYPE ROWS COLS OFFSET INTERP
                    if args.len() != 8 {
                        return bad(format!("`{line}`: an element has eight fields"));
                    }
                    let scalar = match args[3] {
                        "float" => Scalar::Float,
                        "uint" => Scalar::Uint,
                        "int" => Scalar::Int,
                        "bool" => Scalar::Bool,
                        t => return bad(format!("`{line}`: component type `{t}`")),
                    };
                    l.elements.push(Element {
                        region: if key == "v" {
                            Region::Vertex
                        } else {
                            Region::Primitive
                        },
                        name: args[0].to_owned(),
                        index: num(1)?,
                        kind: num(2)?,
                        scalar,
                        rows: num(4)?,
                        cols: num(5)?,
                        offset: num(6)?,
                        interpolation: num(7)?,
                    });
                }
                k => return bad(format!("unknown key `{k}`")),
            }
        }
        l.check()?;
        Ok(l)
    }

    fn check(&self) -> Result<(), LayoutError> {
        let bad = |m: &str| Err(LayoutError(m.into()));
        if self.threads.contains(&0) || self.max_vertices == 0 || self.max_primitives == 0 {
            return bad("no threads or no output");
        }
        if self.max_vertices > 256 {
            // An index is one byte of the packed word.
            return bad("more than 256 vertices (D3D12's limit, and the index byte's)");
        }
        let vend = self.vertex_head + self.max_vertices * self.vertex_stride;
        let pend = self.prim_area + self.max_primitives * self.prim_stride;
        if vend > self.prim_area || pend != self.group_stride {
            return bad("the regions overlap or the group stride is not their end");
        }
        for e in &self.elements {
            let stride = match e.region {
                Region::Vertex => self.vertex_stride,
                Region::Primitive => self.prim_stride,
            };
            let start = if e.region == Region::Primitive { 4 } else { 0 };
            if e.offset < start || e.offset + e.bytes() > stride {
                return bad("an element outside its record");
            }
        }
        Ok(())
    }

    /// Bytes of scratch for `groups` threadgroups.
    pub fn scratch_bytes(&self, groups: u64) -> u64 {
        groups * u64::from(self.group_stride)
    }

    /// Bytes a group writes when it emits `vertices` and `primitives`
    /// (what the memory traffic is made of, not what the scratch holds).
    pub fn written_bytes(&self, vertices: u32, primitives: u32) -> u64 {
        8 + u64::from(vertices) * u64::from(self.vertex_stride)
            + u64::from(primitives) * u64::from(self.prim_stride)
    }

    /// Vertices the draw of one group covers (three per primitive slot).
    pub fn draw_vertices(&self) -> u32 {
        3 * self.max_primitives
    }

    /// The elements of one region.
    pub fn region(&self, r: Region) -> impl Iterator<Item = &Element> {
        self.elements.iter().filter(move |e| e.region == r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "vmcl 1\nthreads 128 1 1\nmaxv 4\nmaxp 2\nvstride 32\npstride 16\n\
vhead 16\nparea 144\ngstride 176\nspace 7\nregister 0\n\
v SV_Position 0 3 float 1 4 0 4\nv COLOR 0 0 float 1 4 16 2\n\
p TEXCOORD 9 0 uint 1 2 4 1\np SV_CullPrimitive 0 30 bool 1 1 12 1\n";

    #[test]
    fn a_layout_file_is_read() {
        let l = Layout::parse(SAMPLE).unwrap();
        assert_eq!(l.threads, [128, 1, 1]);
        assert_eq!(l.elements.len(), 4);
        assert_eq!(l.region(Region::Vertex).count(), 2);
        assert_eq!(l.elements[3].scalar, Scalar::Bool);
        assert_eq!(l.scratch_bytes(10), 1760);
        assert_eq!(l.written_bytes(3, 1), 8 + 96 + 16);
        assert_eq!(l.draw_vertices(), 6);
    }

    #[test]
    fn damaged_layouts_are_refused() {
        assert!(Layout::parse("vmcl 2\n").is_err());
        assert!(Layout::parse(&SAMPLE.replace("gstride 176", "gstride 160")).is_err());
        assert!(Layout::parse(&SAMPLE.replace("maxv 4", "maxv 300")).is_err());
        assert!(Layout::parse(&SAMPLE.replace("float 1 4 16", "float 1 4 24")).is_err());
        assert!(Layout::parse(&format!("{SAMPLE}x y\n")).is_err());
        assert!(Layout::parse(&SAMPLE.replace("float 1 4 0 4", "double 1 4 0 4")).is_err());
    }
}
