//! Helpers of the mesh-limit tests: a mesh function written in MSL with a
//! chosen output (vertex and primitive attributes, maxima, topology, extra
//! threadgroup memory), and a mesh render pipeline built from it with
//! `MTL4Compiler`, so that the point where Metal says no can be found by
//! stepping the size. The device and compiler are the system ones, made here
//! (not the backend's), so that the probe sees what any client sees.
#![allow(dead_code)]
#![cfg(target_vendor = "apple")]

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_foundation::NSString;
use objc2_metal::{
    MTL4Compiler, MTL4CompilerDescriptor, MTL4FunctionDescriptor, MTL4LibraryDescriptor,
    MTL4LibraryFunctionDescriptor, MTL4MeshRenderPipelineDescriptor, MTL4PipelineDescriptor,
    MTLCreateSystemDefaultDevice, MTLDevice, MTLSize,
};

pub struct Probe {
    pub device: Retained<ProtocolObject<dyn MTLDevice>>,
    pub compiler: Retained<ProtocolObject<dyn MTL4Compiler>>,
}

/// Where a refusal came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    /// `MTL4Compiler newLibraryWithDescriptor:` (the MSL front end).
    Library(String),
    /// `newRenderPipelineStateWithDescriptor:` (domain, code, text).
    Pipeline {
        domain: String,
        code: isize,
        text: String,
    },
}

impl Refused {
    pub fn text(&self) -> &str {
        match self {
            Refused::Library(t) => t,
            Refused::Pipeline { text, .. } => text,
        }
    }
    /// The size Metal names in "Total mesh size (N) exceeds ...", if any.
    pub fn total_mesh_size(&self) -> Option<u64> {
        let t = self.text();
        let i = t.find("Total mesh size (")? + "Total mesh size (".len();
        t[i..].split(')').next()?.parse().ok()
    }
}

impl Probe {
    pub fn new() -> Option<Probe> {
        let device = MTLCreateSystemDefaultDevice()?;
        let compiler = device
            .newCompilerWithDescriptor_error(&MTL4CompilerDescriptor::new())
            .ok()?;
        Some(Probe { device, compiler })
    }

    pub fn library(
        &self,
        source: &str,
    ) -> Result<Retained<ProtocolObject<dyn objc2_metal::MTLLibrary>>, Refused> {
        let d = MTL4LibraryDescriptor::new();
        d.setSource(Some(&NSString::from_str(source)));
        self.compiler
            .newLibraryWithDescriptor_error(&d)
            .map_err(|e| Refused::Library(e.localizedDescription().to_string()))
    }

    fn function(
        lib: &ProtocolObject<dyn objc2_metal::MTLLibrary>,
        name: &str,
    ) -> Retained<MTL4FunctionDescriptor> {
        let f = MTL4LibraryFunctionDescriptor::new();
        f.setName(Some(&NSString::from_str(name)));
        f.setLibrary(Some(lib));
        Retained::into_super(f)
    }

    /// A mesh pipeline of `source`'s `mesh_entry` (and `object_entry`), with
    /// `threads` threads per mesh threadgroup. No fragment function.
    pub fn mesh_pipeline(
        &self,
        source: &str,
        mesh_entry: &str,
        object: Option<(&str, usize, usize)>,
        threads: usize,
    ) -> Result<(), Refused> {
        self.mesh_pipeline_with_fragment(source, mesh_entry, object, threads, None)
    }

    /// As [`Probe::mesh_pipeline`], with a fragment function writing one
    /// RGBA8 target.
    pub fn mesh_pipeline_with_fragment(
        &self,
        source: &str,
        mesh_entry: &str,
        object: Option<(&str, usize, usize)>,
        threads: usize,
        fragment: Option<&str>,
    ) -> Result<(), Refused> {
        let lib = self.library(source)?;
        let d = MTL4MeshRenderPipelineDescriptor::new();
        d.setMeshFunctionDescriptor(Some(&Self::function(&lib, mesh_entry)));
        d.setMaxTotalThreadsPerMeshThreadgroup(threads);
        d.setRequiredThreadsPerMeshThreadgroup(MTLSize {
            width: threads,
            height: 1,
            depth: 1,
        });
        if let Some((name, object_threads, payload)) = object {
            d.setObjectFunctionDescriptor(Some(&Self::function(&lib, name)));
            d.setMaxTotalThreadsPerObjectThreadgroup(object_threads);
            d.setPayloadMemoryLength(payload);
        }
        if let Some(f) = fragment {
            d.setFragmentFunctionDescriptor(Some(&Self::function(&lib, f)));
            // SAFETY: attachment 0 exists.
            unsafe {
                d.colorAttachments()
                    .objectAtIndexedSubscript(0)
                    .setPixelFormat(objc2_metal::MTLPixelFormat::RGBA8Unorm);
            }
        }
        let base: &MTL4PipelineDescriptor = &d;
        match self
            .compiler
            .newRenderPipelineStateWithDescriptor_compilerTaskOptions_error(base, None)
        {
            Ok(_) => Ok(()),
            Err(e) => Err(Refused::Pipeline {
                domain: e.domain().to_string(),
                code: e.code(),
                text: e.localizedDescription().to_string(),
            }),
        }
    }
}

/// The shape of a mesh output.
#[derive(Clone, Debug)]
pub struct Shape {
    /// MSL types of the vertex attributes after `[[position]]` (`float4`,
    /// `float2`, `half4`, `uint`, ...). Position is always a `float4`.
    pub vertex: Vec<&'static str>,
    /// MSL types of the per-primitive attributes.
    pub primitive: Vec<&'static str>,
    pub max_vertices: u32,
    pub max_primitives: u32,
    /// `triangle`, `line` or `point`.
    pub topology: &'static str,
    /// Bytes of `threadgroup` memory the mesh function declares and uses.
    pub threadgroup_bytes: u32,
    pub threads: u32,
}

impl Shape {
    pub fn new(vertex: Vec<&'static str>, nv: u32, np: u32) -> Shape {
        Shape {
            vertex,
            primitive: Vec::new(),
            max_vertices: nv,
            max_primitives: np,
            topology: "triangle",
            threadgroup_bytes: 0,
            threads: 32,
        }
    }

    /// MSL source: `ms` writes every attribute of every vertex and primitive.
    pub fn msl(&self) -> String {
        let mut s = String::from(
            "#include <metal_stdlib>\n#include <metal_mesh>\nusing namespace metal;\n",
        );
        s += "struct V { float4 pos [[position]];";
        for (i, t) in self.vertex.iter().enumerate() {
            s += &format!(" {t} a{i};");
        }
        s += " };\nstruct P {";
        for (i, t) in self.primitive.iter().enumerate() {
            s += &format!(" {t} p{i};");
        }
        if self.primitive.is_empty() {
            s += " ";
        }
        s += "};\n";
        let prim_t = if self.primitive.is_empty() {
            "void"
        } else {
            "P"
        };
        s += &format!(
            "using M = mesh<V, {prim_t}, {}, {}, topology::{}>;\n",
            self.max_vertices, self.max_primitives, self.topology
        );
        let per_prim = match self.topology {
            "triangle" => 3,
            "line" => 2,
            _ => 1,
        };
        if self.threadgroup_bytes > 0 {
            s += &format!(
                "struct Shared {{ float v[{}]; }};\n",
                self.threadgroup_bytes / 4
            );
        }
        s += &format!(
            "[[mesh, max_total_threads_per_threadgroup({})]]\nvoid ms(M m, uint tid [[thread_index_in_threadgroup]], \
             uint2 gid [[threadgroup_position_in_grid]]",
            self.threads
        );
        s += ") {\n";
        if self.threadgroup_bytes > 0 {
            s += "  threadgroup Shared sh;\n";
            s += &format!(
                "  for (uint i = tid; i < {}; i += {}) sh.v[i] = float(i + gid.x);\n  threadgroup_barrier(mem_flags::mem_threadgroup);\n",
                self.threadgroup_bytes / 4,
                self.threads
            );
        }
        s += &format!(
            "  if (tid == 0) m.set_primitive_count({});\n",
            self.max_primitives
        );
        s += &format!(
            "  for (uint v = tid; v < {}; v += {}) {{ V o; o.pos = float4(float(v), float(gid.x), 0, 1);\n",
            self.max_vertices, self.threads
        );
        for (i, t) in self.vertex.iter().enumerate() {
            s += &format!("    o.a{i} = {t}(float(v + {i}));\n");
        }
        s += "    m.set_vertex(v, o); }\n";
        s += &format!(
            "  for (uint p = tid; p < {}; p += {}) {{\n",
            self.max_primitives, self.threads
        );
        for k in 0..per_prim {
            s += &format!(
                "    m.set_index(p * {per_prim} + {k}, (p + {k}) % {});\n",
                self.max_vertices
            );
        }
        if !self.primitive.is_empty() {
            s += "    P q;\n";
            for (i, t) in self.primitive.iter().enumerate() {
                s += &format!("    q.p{i} = {t}(float(p + {i}));\n");
            }
            s += "    m.set_primitive(p, q);\n";
        }
        s += "  }\n";
        if self.threadgroup_bytes > 0 {
            s += "  if (tid == 0) m.set_vertex(0, [&]{ V o; o.pos = float4(sh.v[0]); ";
            for (i, t) in self.vertex.iter().enumerate() {
                s += &format!("o.a{i} = {t}(sh.v[{i}]); ");
            }
            s += "return o; }());\n";
        }
        s += "}\n";
        s
    }

    /// Builds the pipeline.
    pub fn build(&self, probe: &Probe) -> Result<(), Refused> {
        probe.mesh_pipeline(&self.msl(), "ms", None, self.threads as usize)
    }
}
