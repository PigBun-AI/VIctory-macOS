//! mac S6: the hardware host of victory-shader's tests on macOS.
//!
//! The tests of `valu_tests`, `wave64_tests`, ... are written against
//! [`Host`] (`host.rs`: D3D12 on Windows). On macOS the same names are this
//! module: a [`Host`] that sends the translated DXIL and the test's bindings
//! to an **executor process**, `victory-shader-exec` of the Metal back
//! (`metal/shader-exec`, branch `harness-back`; DXIL -> Metal Shader
//! Converter -> Metal 4 compute -> read back), and returns what it read.
//!
//! The process is named by `VICTORY_SHADER_EXEC`; without it [`Host::new`]
//! fails, which the tests treat as "no device" and skip, so `cargo test` of
//! this crate stays green on a Mac with no Metal tooling. This crate names no
//! Metal symbol and depends on no `victory-metal-*` crate (the layering
//! rule); the pipe protocol is the whole contract (`wire`, mirrored in
//! `metal/shader-exec/src/wire.rs`; golden vectors pin both).
//!
//! One executor serves a whole test process (a crash or hang kills it, the
//! next request starts a new one; the request that crashed it fails and
//! says so). `VICTORY_SHADER_EXEC_LOG=<file>` appends one line per request
//! (outcome, test thread, text), the data of the pass/fail/skip tally
//! (`tools/mac/shader-hw.sh`).

use std::io::{Read, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use super::HostShader;
use crate::dxbc::{Container, FourCC};

/// The D3D12 and DXGI names the tests use, as plain values (the Windows
/// host re-exports the `windows` crate's own under the same names).
#[allow(non_camel_case_types, non_upper_case_globals)]
pub mod api {
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct D3D12_FILTER(pub u32);
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct D3D12_TEXTURE_ADDRESS_MODE(pub u32);
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct D3D12_COMPARISON_FUNC(pub u32);
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct DXGI_FORMAT(pub u32);

    pub const D3D12_FILTER_MIN_MAG_MIP_POINT: D3D12_FILTER = D3D12_FILTER(0);
    pub const D3D12_FILTER_MIN_MAG_MIP_LINEAR: D3D12_FILTER = D3D12_FILTER(0x15);
    pub const D3D12_TEXTURE_ADDRESS_MODE_WRAP: D3D12_TEXTURE_ADDRESS_MODE = D3D12_TEXTURE_ADDRESS_MODE(1);
    pub const D3D12_TEXTURE_ADDRESS_MODE_CLAMP: D3D12_TEXTURE_ADDRESS_MODE = D3D12_TEXTURE_ADDRESS_MODE(3);

    pub const DXGI_FORMAT_R16G16B16A16_FLOAT: DXGI_FORMAT = DXGI_FORMAT(10);
    pub const DXGI_FORMAT_R16G16B16A16_UINT: DXGI_FORMAT = DXGI_FORMAT(12);
    pub const DXGI_FORMAT_R16G16B16A16_SINT: DXGI_FORMAT = DXGI_FORMAT(14);
    pub const DXGI_FORMAT_R10G10B10A2_UNORM: DXGI_FORMAT = DXGI_FORMAT(24);
    pub const DXGI_FORMAT_R8G8B8A8_UNORM: DXGI_FORMAT = DXGI_FORMAT(28);
    pub const DXGI_FORMAT_R8G8B8A8_UINT: DXGI_FORMAT = DXGI_FORMAT(30);
    pub const DXGI_FORMAT_R8G8B8A8_SNORM: DXGI_FORMAT = DXGI_FORMAT(31);
    pub const DXGI_FORMAT_R16G16_SNORM: DXGI_FORMAT = DXGI_FORMAT(37);
    pub const DXGI_FORMAT_R32_UINT: DXGI_FORMAT = DXGI_FORMAT(42);
    pub const DXGI_FORMAT_R8_UNORM: DXGI_FORMAT = DXGI_FORMAT(61);
    pub const DXGI_FORMAT_R16_FLOAT: DXGI_FORMAT = DXGI_FORMAT(54);
    pub const DXGI_FORMAT_R16_UINT: DXGI_FORMAT = DXGI_FORMAT(57);
    pub const DXGI_FORMAT_R16_SINT: DXGI_FORMAT = DXGI_FORMAT(59);

    #[derive(Clone, Copy, Debug, Default, PartialEq)]
    pub struct D3D12_SAMPLER_DESC {
        pub Filter: D3D12_FILTER,
        pub AddressU: D3D12_TEXTURE_ADDRESS_MODE,
        pub AddressV: D3D12_TEXTURE_ADDRESS_MODE,
        pub AddressW: D3D12_TEXTURE_ADDRESS_MODE,
        pub MipLODBias: f32,
        pub MaxAnisotropy: u32,
        pub ComparisonFunc: D3D12_COMPARISON_FUNC,
        pub BorderColor: [f32; 4],
        pub MinLOD: f32,
        pub MaxLOD: f32,
    }

    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct D3D12_SHADER_VISIBILITY(pub u32);
    pub const D3D12_SHADER_VISIBILITY_ALL: D3D12_SHADER_VISIBILITY = D3D12_SHADER_VISIBILITY(0);
    pub const D3D12_SHADER_VISIBILITY_VERTEX: D3D12_SHADER_VISIBILITY = D3D12_SHADER_VISIBILITY(1);
    pub const D3D12_SHADER_VISIBILITY_PIXEL: D3D12_SHADER_VISIBILITY = D3D12_SHADER_VISIBILITY(5);
    pub const D3D12_SHADER_VISIBILITY_MESH: D3D12_SHADER_VISIBILITY = D3D12_SHADER_VISIBILITY(7);

    pub use super::{Pso as ID3D12PipelineState, RootSig as ID3D12RootSignature};
}

/// What the tests ask of the device: the numbers D3D12's feature queries
/// give on Windows ([`Host::caps`]).
#[derive(Clone, Debug, Default)]
pub struct HostCaps {
    pub name: String,
    pub wave_lane_min: u32,
    pub wave_lane_max: u32,
    /// An AMD adapter (the console's own kind of hardware).
    pub amd: bool,
    pub mesh_shaders: bool,
    pub barycentrics: bool,
}

// ---- the pipe protocol (mirrors metal/shader-exec/src/{wire,req}.rs) ----

mod wire {
    pub const VERSION: u32 = 1;
    pub const OP_HELLO: u8 = 1;
    pub const OP_COMPILE: u8 = 2;
    pub const OP_DISPATCH: u8 = 3;
    pub const OP_DRAW: u8 = 4;
    pub const OP_PIPELINE: u8 = 5;
    pub const OK: u8 = 0;
    pub const UNSUPPORTED: u8 = 1;

    #[derive(Default)]
    pub struct W(pub Vec<u8>);
    impl W {
        pub fn u8(&mut self, v: u8) {
            self.0.push(v);
        }
        pub fn u32(&mut self, v: u32) {
            self.0.extend_from_slice(&v.to_le_bytes());
        }
        pub fn f32(&mut self, v: f32) {
            self.u32(v.to_bits());
        }
        pub fn bytes(&mut self, v: &[u8]) {
            self.u32(v.len() as u32);
            self.0.extend_from_slice(v);
        }
        pub fn u32s(&mut self, v: &[u32]) {
            self.u32(v.len() as u32);
            for x in v {
                self.u32(*x);
            }
        }
        pub fn f32s(&mut self, v: &[f32]) {
            self.u32(v.len() as u32);
            for x in v {
                self.f32(*x);
            }
        }
    }

    pub struct R<'a>(pub &'a [u8], pub usize);
    impl<'a> R<'a> {
        fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
            let end = self.1.checked_add(n).filter(|&e| e <= self.0.len()).ok_or("short reply")?;
            let s = &self.0[self.1..end];
            self.1 = end;
            Ok(s)
        }
        pub fn u8(&mut self) -> Result<u8, String> {
            Ok(self.take(1)?[0])
        }
        pub fn u32(&mut self) -> Result<u32, String> {
            Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
        }
        pub fn str(&mut self) -> Result<String, String> {
            let n = self.u32()? as usize;
            Ok(String::from_utf8_lossy(self.take(n)?).into_owned())
        }
        pub fn u32s(&mut self) -> Result<Vec<u32>, String> {
            let n = self.u32()? as usize;
            Ok(self
                .take(n.checked_mul(4).ok_or("overflow")?)?
                .chunks_exact(4)
                .map(|c| u32::from_le_bytes(c.try_into().unwrap()))
                .collect())
        }
    }
}

use wire::{R, W};

fn put_root(w: &mut W, flags: u32, params: &[(u8, u8, u32, u32, u32)], samplers: &[(super::StaticSampler, u32, u32, u8)]) {
    w.u32(flags);
    w.u32(params.len() as u32);
    for &(kind, vis, register, space, n32) in params {
        w.u8(kind);
        w.u8(vis);
        w.u32(register);
        w.u32(space);
        w.u32(n32);
    }
    w.u32(samplers.len() as u32);
    for (s, register, space, vis) in samplers {
        w.u32(s.filter);
        for a in s.address {
            w.u32(a);
        }
        w.f32(s.mip_lod_bias);
        w.u32(s.max_anisotropy);
        w.u32(s.comparison);
        w.u32(s.border);
        w.f32(s.min_lod);
        w.f32(s.max_lod);
        w.u32(*register);
        w.u32(*space);
        w.u8(*vis);
    }
}

// ---- the executor process ----

struct Worker {
    child: Child,
    stdin: ChildStdin,
    replies: Receiver<Vec<u8>>,
}

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

static WORKER: Mutex<Option<Worker>> = Mutex::new(None);
static CAPS: OnceLock<Result<HostCaps, String>> = OnceLock::new();

fn log(outcome: &str, what: &str, text: &str) {
    let Some(path) = std::env::var_os("VICTORY_SHADER_EXEC_LOG") else {
        return;
    };
    let thread = std::thread::current();
    let line = format!(
        "{outcome}\t{what}\t{}\t{}\n",
        thread.name().unwrap_or("?"),
        text.replace(['\n', '\t'], " ")
    );
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = f.write_all(line.as_bytes());
    }
}

fn spawn() -> Result<Worker, String> {
    let path = std::env::var_os("VICTORY_SHADER_EXEC")
        .ok_or("VICTORY_SHADER_EXEC is not set (tools/mac/shader-hw.sh builds and names the executor)")?;
    let mut child = Command::new(&path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| format!("cannot start {}: {e}", path.to_string_lossy()))?;
    let stdin = child.stdin.take().ok_or("no stdin")?;
    let mut stdout = child.stdout.take().ok_or("no stdout")?;
    let (tx, replies) = channel();
    std::thread::spawn(move || {
        loop {
            let mut len = [0u8; 4];
            if stdout.read_exact(&mut len).is_err() {
                return;
            }
            let mut buf = vec![0u8; u32::from_le_bytes(len) as usize];
            if stdout.read_exact(&mut buf).is_err() || tx.send(buf).is_err() {
                return;
            }
        }
    });
    Ok(Worker {
        child,
        stdin,
        replies,
    })
}

fn timeout() -> Duration {
    let secs = std::env::var("VICTORY_SHADER_EXEC_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(120);
    Duration::from_secs(secs)
}

/// One request, one reply body (after the status byte). `Err(text)`: the
/// executor said so, died, or hung.
fn call(what: &str, request: &[u8]) -> Result<Vec<u8>, String> {
    let mut guard = WORKER.lock().unwrap_or_else(|e| e.into_inner());
    if guard.is_none() {
        *guard = Some(spawn()?);
    }
    let w = guard.as_mut().expect("spawned");
    let sent = w
        .stdin
        .write_all(&(request.len() as u32).to_le_bytes())
        .and_then(|()| w.stdin.write_all(request))
        .and_then(|()| w.stdin.flush());
    let reply = match sent {
        Err(e) => Err(format!("the executor is gone ({e})")),
        Ok(()) => match w.replies.recv_timeout(timeout()) {
            Ok(r) => Ok(r),
            Err(RecvTimeoutError::Timeout) => Err(format!("the executor hung for {:?}", timeout())),
            Err(RecvTimeoutError::Disconnected) => {
                let status = w.child.try_wait().ok().flatten();
                Err(format!("the executor died ({status:?})"))
            }
        },
    };
    let reply = match reply {
        Ok(r) => r,
        Err(e) => {
            // A dead or hung executor is replaced by the next request.
            *guard = None;
            log("crash", what, &e);
            return Err(format!("crash: {e}"));
        }
    };
    let mut r = R(&reply, 0);
    let status = r.u8()?;
    if status == wire::OK {
        log("ok", what, "");
        return Ok(reply[1..].to_vec());
    }
    let text = r.str().unwrap_or_default();
    if status == wire::UNSUPPORTED {
        log("unsupported", what, &text);
        Err(format!("unsupported: {text}"))
    } else {
        log("failed", what, &text);
        Err(text)
    }
}

// ---- the host ----

/// The device the tests run on: here, the executor behind a pipe.
pub struct Host {
    caps: HostCaps,
}

/// A root signature: the translator's host declares it from the shader.
#[derive(Clone)]
pub struct RootSig {
    wire: Vec<u8>,
}

/// A pipeline: the DXIL (the executor converts and caches it by content).
#[derive(Clone)]
pub struct Pso {
    dxil: Vec<u8>,
    /// The rest of a graphics pipeline (none for a compute one).
    gfx: Option<Gfx>,
}

/// What a graphics pipeline holds besides its first stage (`Pso::dxil`: the vertex or mesh
/// shader): the optional geometry shader and the pixel shader, the render targets' DXGI formats,
/// the depth target (0 none, 1 D32_FLOAT, 2 with stencil) and the vertex inputs (name, index,
/// DXGI format, byte offset).
#[derive(Clone)]
struct Gfx {
    mesh: bool,
    gs: Vec<u8>,
    ps: Vec<u8>,
    targets: Vec<u32>,
    depth: u8,
    inputs: Vec<(String, u32, u32, u32)>,
}

impl Host {
    /// The executor's device. `warp` has no meaning here: there is no
    /// software adapter on a Mac, so it fails like a Windows machine without
    /// WARP and the test skips.
    pub fn new(warp: bool) -> Result<Self, String> {
        if warp || std::env::var("VICTORY_WARP").is_ok_and(|v| v == "1") {
            let e = "no software adapter on macOS".to_owned();
            log("no-host", "new(warp)", &e);
            return Err(e);
        }
        let caps = CAPS.get_or_init(|| {
            let reply = call("hello", &[wire::OP_HELLO])?;
            let mut r = R(&reply, 0);
            let version = r.u32()?;
            if version != wire::VERSION {
                return Err(format!("the executor speaks protocol {version}, this crate {}", wire::VERSION));
            }
            let name = r.str()?;
            let (wave_lane_min, wave_lane_max, flags) = (r.u32()?, r.u32()?, r.u32()?);
            Ok(HostCaps {
                name,
                wave_lane_min,
                wave_lane_max,
                amd: false,
                mesh_shaders: flags & 2 != 0,
                barycentrics: flags & 4 != 0,
            })
        });
        match caps {
            Ok(caps) => Ok(Host { caps: caps.clone() }),
            Err(e) => {
                log("no-host", "new", e);
                Err(e.clone())
            }
        }
    }

    pub fn caps(&self) -> HostCaps {
        self.caps.clone()
    }

    /// The root signature of these stages' shaders (compute here: one stage,
    /// visible to all): user data is parameter 0, root views follow, the
    /// shaders' static samplers last; both heaps directly indexed.
    pub fn root_signature_for(&self, stages: &[(&HostShader, api::D3D12_SHADER_VISIBILITY)]) -> Result<RootSig, String> {
        let mut params: Vec<(u8, u8, u32, u32, u32)> = stages
            .iter()
            .map(|&(s, vis)| (0, vis.0 as u8, 0, super::USER_DATA_SPACE, s.user_data_dwords().max(1)))
            .collect();
        let mut views: Vec<(u8, u8)> = stages
            .iter()
            .flat_map(|(s, _)| {
                let cbvs = s.root_cbvs.iter().map(|&r| (r, 0));
                let srvs = s.root_srvs.iter().map(|&r| (r, 1));
                cbvs.chain(srvs).chain(s.root_uavs.iter().map(|&r| (r, 2)))
            })
            .collect();
        views.sort_unstable();
        views.dedup();
        for (slot, kind) in views {
            // wire kinds: 1 CBV, 2 SRV, 3 UAV
            params.push((kind + 1, 0, u32::from(slot), super::ROOT_CBV_SPACE, 0));
        }
        let mut samplers = Vec::new();
        for &(s, vis) in stages {
            for (n, d) in s.static_samplers.iter().enumerate() {
                // The device takes anisotropic point-mip filtering.
                samplers.push((*d, n as u32, super::SAMPLER_SPACE, vis.0 as u8));
            }
        }
        let mut w = W::default();
        put_root(&mut w, 3, &params, &samplers);
        Ok(RootSig { wire: w.0 })
    }

    /// A compute pipeline; user data is root parameter 0. The executor
    /// converts it now, so a program MSC refuses is an error here, as a
    /// pipeline the D3D12 device refuses is.
    pub fn compute_pipeline(&self, cs: &HostShader) -> Result<(RootSig, Pso), String> {
        let root = self.root_signature_for(&[(cs, api::D3D12_SHADER_VISIBILITY_ALL)])?;
        let pso = self.compute_pipeline_with(&root, &cs.dxil)?;
        Ok((root, pso))
    }

    pub fn compute_pipeline_with(&self, root: &RootSig, cs: &[u8]) -> Result<Pso, String> {
        let mut w = W::default();
        w.u8(wire::OP_COMPILE);
        w.0.extend_from_slice(&root.wire);
        w.bytes(cs);
        call("compile", &w.0)?;
        Ok(Pso { dxil: cs.to_vec(), gfx: None })
    }

    /// Runs a compute pipeline and returns each UAV's contents in heap
    /// order (texels as float bits, row by row).
    pub fn dispatch(&self, root: &RootSig, pso: &Pso, d: &Dispatch) -> Result<Vec<Vec<u32>>, String> {
        let mut w = W::default();
        w.u8(wire::OP_DISPATCH);
        w.0.extend_from_slice(&root.wire);
        w.bytes(&pso.dxil);
        put_bindings(&mut w, &d.roots, d.views, d.samplers, d.groups)?;
        let reply = call("dispatch", &w.0)?;
        let mut r = R(&reply, 0);
        let n = r.u32()?;
        (0..n).map(|_| r.u32s()).collect()
    }

    /// The root signature of a vertex and a pixel shader: user data of the vertex stage is root
    /// parameter 0, the pixel stage's 1.
    pub fn root_signature(&self, vs: &HostShader, ps: &HostShader) -> Result<RootSig, String> {
        self.root_signature_for(&[
            (vs, api::D3D12_SHADER_VISIBILITY_VERTEX),
            (ps, api::D3D12_SHADER_VISIBILITY_PIXEL),
        ])
    }

    /// A graphics pipeline from a vertex and a pixel shader.
    pub fn pipeline(&self, vs: &HostShader, ps: &HostShader) -> Result<(RootSig, Pso), String> {
        let root = self.root_signature(vs, ps)?;
        let pso = self.pipeline_with(&root, &vs.dxil, &ps.dxil)?;
        Ok((root, pso))
    }

    pub fn pipeline_with(&self, root: &RootSig, vs: &[u8], ps: &[u8]) -> Result<Pso, String> {
        self.pipeline_with_gs(root, vs, None, ps)
    }

    /// As [`Host::pipeline_with`], with a geometry shader between the two.
    pub fn pipeline_with_gs(&self, root: &RootSig, vs: &[u8], gs: Option<&[u8]>, ps: &[u8]) -> Result<Pso, String> {
        let inputs = sig(vs, FourCC::ISG1)?;
        let mut offset = 0;
        let layout = inputs
            .iter()
            .filter(|e| e.kind == 0)
            .map(|e| {
                let at = offset;
                offset += 4 * u32::from(e.cols.clamp(1, 4));
                (e.name.clone(), e.index, input_format(e), at)
            })
            .collect();
        self.make_pipeline(root, vs, gs.unwrap_or(&[]), ps, false, layout)
    }

    /// A mesh pipeline from a mesh and a pixel shader (user data of the mesh stage is root
    /// parameter 0, the pixel stage's 1).
    pub fn mesh_pipeline(&self, ms: &HostShader, ps: &HostShader) -> Result<(RootSig, Pso), String> {
        let root = self.root_signature_for(&[
            (ms, api::D3D12_SHADER_VISIBILITY_MESH),
            (ps, api::D3D12_SHADER_VISIBILITY_PIXEL),
        ])?;
        let pso = self.mesh_pipeline_with(&root, &ms.dxil, &ps.dxil)?;
        Ok((root, pso))
    }

    pub fn mesh_pipeline_with(&self, root: &RootSig, ms: &[u8], ps: &[u8]) -> Result<Pso, String> {
        self.make_pipeline(root, ms, &[], ps, true, Vec::new())
    }

    /// Sends the pipeline to the executor now, so a pair MSC or Metal refuses is an error here as
    /// it is from D3D12's `CreateGraphicsPipelineState`.
    fn make_pipeline(
        &self,
        root: &RootSig,
        first: &[u8],
        gs: &[u8],
        ps: &[u8],
        mesh: bool,
        inputs: Vec<(String, u32, u32, u32)>,
    ) -> Result<Pso, String> {
        let outputs = sig(ps, FourCC::OSG1)?;
        let mut targets: Vec<u32> = Vec::new();
        for o in outputs.iter().filter(|o| o.kind == 16) {
            let n = o.row.unwrap_or(0) as usize;
            if targets.len() <= n {
                targets.resize(n + 1, 2);
            }
            targets[n] = match o.comp {
                9 => 2,
                4 => 4,
                _ => 3,
            };
        }
        let depth = if outputs.iter().any(|o| o.kind == 20) {
            2
        } else {
            u8::from(outputs.iter().any(|o| matches!(o.kind, 17..=19)))
        };
        let gfx = Gfx { mesh, gs: gs.to_vec(), ps: ps.to_vec(), targets, depth, inputs };
        let mut w = W::default();
        w.u8(wire::OP_PIPELINE);
        put_pipeline(&mut w, root, first, &gfx);
        call("pipeline", &w.0)?;
        Ok(Pso { dxil: first.to_vec(), gfx: Some(gfx) })
    }

    /// Draws one triangle into an RGBA32F target, or dispatches `mesh_groups` mesh groups, and
    /// returns the target's texels (an integer target's come back as their bits).
    pub fn draw(&self, root: &RootSig, pso: &Pso, d: &Draw) -> Result<Vec<[f32; 4]>, String> {
        let gfx = pso.gfx.as_ref().ok_or("a compute pipeline in a draw")?;
        let mut w = W::default();
        w.u8(wire::OP_DRAW);
        put_pipeline(&mut w, root, &pso.dxil, gfx);
        w.f32s(d.vertices);
        w.u32(d.stride);
        w.u32(d.size.0);
        w.u32(d.size.1);
        let views: Vec<View> = d.textures.iter().map(|&(wd, h, t)| View::Texture(wd, h, t)).collect();
        put_bindings(&mut w, &d.roots, &views, d.samplers, [d.mesh_groups, 1, 1])?;
        let reply = call("draw", &w.0)?;
        let mut r = R(&reply, 0);
        let texels = r.u32s()?;
        Ok(texels
            .chunks_exact(4)
            .map(|c| [f32::from_bits(c[0]), f32::from_bits(c[1]), f32::from_bits(c[2]), f32::from_bits(c[3])])
            .collect())
    }
}

/// The root arguments, the view heap, the sampler heap and the group counts of a dispatch or a draw.
fn put_bindings(
    w: &mut W,
    roots: &[(u32, Root)],
    views: &[View],
    samplers: &[api::D3D12_SAMPLER_DESC],
    groups: [u32; 3],
) -> Result<(), String> {
        w.u32(roots.len() as u32);
        for (param, arg) in roots {
            w.u32(*param);
            match arg {
                Root::Constants(c) => {
                    w.u8(0);
                    w.u32s(c);
                }
                Root::View { srv, data } => {
                    w.u8(1);
                    w.u8(u8::from(*srv));
                    w.u32s(data);
                }
                Root::Uav(n) => {
                    w.u8(2);
                    w.u32(*n);
                }
                Root::Table { .. } => return Err("unsupported: root descriptor tables".into()),
            }
        }
        w.u32(views.len() as u32);
        for v in views {
            match v {
                View::Texture(wd, h, t) => {
                    w.u8(0);
                    w.u32(*wd);
                    w.u32(*h);
                    w.f32s(&t.iter().flatten().copied().collect::<Vec<_>>());
                }
                View::Cube(size, mips, t) => {
                    w.u8(1);
                    w.u32(*size);
                    w.u32(u32::from(*mips));
                    w.f32s(&t.iter().flatten().copied().collect::<Vec<_>>());
                }
                View::Depth(wd, h, d) => {
                    w.u8(9);
                    w.u32(*wd);
                    w.u32(*h);
                    w.f32s(d);
                }
                View::RawUav(n) => {
                    w.u8(2);
                    w.u32(*n);
                }
                View::TextureUav(a, b) => {
                    w.u8(3);
                    w.u32(*a);
                    w.u32(*b);
                }
                View::UintTextureUav(a, b) => {
                    w.u8(4);
                    w.u32(*a);
                    w.u32(*b);
                }
                View::HalfUav(n) => {
                    w.u8(5);
                    w.u32(*n);
                }
                View::TypedUav { data, format, elem_bytes } => {
                    w.u8(6);
                    w.u32s(data);
                    w.u32(format.0);
                    w.u32(*elem_bytes);
                }
                View::Cbv(data) => {
                    w.u8(7);
                    w.u32s(data);
                }
                View::Buffer { data, stride } => {
                    w.u8(8);
                    w.u32s(data);
                    match stride {
                        Stride::Raw => w.u8(0),
                        Stride::Typed => w.u8(1),
                        Stride::Structured(s) => {
                            w.u8(2);
                            w.u32(*s);
                        }
                        Stride::Format(f, bytes) => {
                            w.u8(3);
                            w.u32(f.0);
                            w.u32(*bytes);
                        }
                    }
                }
            }
        }
        w.u32(samplers.len() as u32);
        for s in samplers {
            w.u32(s.Filter.0);
            w.u32(s.AddressU.0);
            w.u32(s.AddressV.0);
            w.u32(s.AddressW.0);
            w.f32(s.MipLODBias);
            w.u32(s.MaxAnisotropy);
            w.u32(s.ComparisonFunc.0);
            for c in s.BorderColor {
                w.f32(c);
            }
            w.f32(s.MinLOD);
            w.f32(s.MaxLOD);
        }
        for g in groups {
            w.u32(g);
        }
    Ok(())
}

fn sig(dxil: &[u8], fourcc: FourCC) -> Result<Vec<super::container::SigElement>, String> {
    let c = Container::parse(dxil).map_err(|e| e.to_string())?;
    match c.part(fourcc) {
        Some(p) => super::signature(p.data).map_err(|e| e.to_string()),
        None => Ok(Vec::new()),
    }
}

/// The DXGI format number of a vertex input (a vector of 32-bit floats, signed or unsigned).
fn input_format(e: &super::container::SigElement) -> u32 {
    let i = usize::from(e.cols.clamp(1, 4)) - 1;
    match e.comp {
        9 => [41, 16, 6, 2][i],
        4 => [43, 18, 8, 4][i],
        _ => [42, 17, 7, 3][i],
    }
}

fn put_pipeline(w: &mut W, root: &RootSig, first: &[u8], g: &Gfx) {
    w.0.extend_from_slice(&root.wire);
    w.u8(u8::from(g.mesh));
    w.bytes(first);
    w.bytes(&g.gs);
    w.bytes(&g.ps);
    w.u32(g.targets.len() as u32);
    for t in &g.targets {
        w.u32(*t);
    }
    w.u8(g.depth);
    w.u32(g.inputs.len() as u32);
    for (name, index, format, offset) in &g.inputs {
        w.bytes(name.as_bytes());
        w.u32(*index);
        w.u32(*format);
        w.u32(*offset);
    }
}

/// Bindings for [`Host::draw`]: root arguments and the heap contents.
pub struct Draw<'a> {
    pub roots: Vec<(u32, Root)>,
    /// RGBA32F textures (width, height, texels) at view-heap indices 0..
    pub textures: &'a [(u32, u32, &'a [[f32; 4]])],
    /// Samplers at sampler-heap indices 0..
    pub samplers: &'a [api::D3D12_SAMPLER_DESC],
    /// Three vertices of stride bytes (none for position-free layouts).
    pub vertices: &'a [f32],
    pub stride: u32,
    pub size: (u32, u32),
    /// Mesh thread groups to dispatch instead of drawing (0: draw).
    pub mesh_groups: u32,
}

// (The Windows host's `stub_vs`/`stub_ps` serve the container-hash test and `diff_tests`, which stay on Windows.)

/// A root argument for [`Host::dispatch`].
pub enum Root {
    Constants(Vec<u32>),
    /// Not on this host (the executor binds through the heap indices).
    Table { sampler: bool, index: usize },
    /// A root CBV (or with `srv`, a root SRV) over a buffer of these dwords.
    View { srv: bool, data: Vec<u32> },
    /// A root UAV over a zeroed buffer of this many dwords (not read back).
    Uav(u32),
}

/// A view in the heap for [`Host::dispatch`].
pub enum View<'a> {
    /// An RGBA32F texture SRV (width, height, texels).
    Texture(u32, u32, &'a [[f32; 4]]),
    /// An RGBA32F cube SRV (face size, mip count, texels in D3D subresource order).
    Cube(u32, u16, &'a [[f32; 4]]),
    /// A depth texture SRV (width, height, depth values; D32_FLOAT read as R32_FLOAT): what a
    /// comparison sampler reads.
    Depth(u32, u32, &'a [f32]),
    /// A raw buffer UAV of this many dwords, zeroed.
    RawUav(u32),
    /// An RGBA32F texture UAV (width, height), zeroed.
    TextureUav(u32, u32),
    /// An R32_UINT texture UAV (width, height), zeroed (atomics).
    UintTextureUav(u32, u32),
    /// A typed RGBA16F buffer UAV of this many elements, zeroed.
    HalfUav(u32),
    /// A typed buffer UAV pre-filled with `data` (`format`, bytes per element).
    TypedUav {
        data: &'a [u32],
        format: api::DXGI_FORMAT,
        elem_bytes: u32,
    },
    /// A constant buffer of these dwords.
    Cbv(&'a [u32]),
    /// A buffer SRV over these dwords.
    Buffer { data: &'a [u32], stride: Stride },
}

/// How [`View::Buffer`] is viewed.
#[derive(Clone, Copy)]
pub enum Stride {
    Raw,
    /// RGBA32F elements.
    Typed,
    /// Structured, elements of this many bytes.
    Structured(u32),
    /// Typed, elements in this format of this many bytes.
    Format(api::DXGI_FORMAT, u32),
}

/// Bindings for [`Host::dispatch`]: root arguments, the view heap in order,
/// the sampler heap, and the group counts.
pub struct Dispatch<'a> {
    pub roots: Vec<(u32, Root)>,
    pub views: &'a [View<'a>],
    pub samplers: &'a [api::D3D12_SAMPLER_DESC],
    pub groups: [u32; 3],
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bytes of a root signature, pinned (the executor decodes the same
    /// vector in `metal/shader-exec/src/req.rs`).
    #[test]
    fn the_root_wire_form_is_pinned() {
        let mut w = W::default();
        put_root(&mut w, 3, &[(0, 0, 0, 7, 4), (3, 0, 2, 9, 0)], &[]);
        assert_eq!(
            w.0,
            [
                3, 0, 0, 0, // flags
                2, 0, 0, 0, // params
                0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 4, 0, 0, 0, // kind 0 vis 0 reg 0 space 7 n32 4
                3, 0, 2, 0, 0, 0, 9, 0, 0, 0, 0, 0, 0, 0, // kind 3 vis 0 reg 2 space 9 n32 0
                0, 0, 0, 0, // samplers
            ]
        );
    }

    /// The same bytes as `metal/shader-exec/src/req.rs` (`the_pipeline_wire_form_matches_the_hosts`).
    #[test]
    fn the_pipeline_wire_form_is_pinned() {
        let mut w = W::default();
        let g = Gfx {
            mesh: true,
            gs: Vec::new(),
            ps: b"c".to_vec(),
            targets: vec![2],
            depth: 0,
            inputs: vec![("P".into(), 0, 6, 0)],
        };
        put_pipeline(&mut w, &RootSig { wire: Vec::new() }, b"ab", &g);
        assert_eq!(
            w.0,
            [
                1, 2, 0, 0, 0, b'a', b'b', 0, 0, 0, 0, 1, 0, 0, 0, b'c', 1, 0, 0, 0, 2, 0, 0, 0, 0, 1, 0, 0, 0, 1,
                0, 0, 0, b'P', 0, 0, 0, 0, 6, 0, 0, 0, 0, 0, 0, 0
            ]
        );
    }
}
