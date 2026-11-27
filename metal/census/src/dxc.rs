//! The default partner stages a pipeline needs (a pixel shader for a vertex
//! or mesh shader, a vertex shader for a pixel shader), made with DXC from
//! HLSL of our own: `tools/macos/dxc-d3dmetal.sh`'s front end over D3DMetal's
//! x86_64 `libdxcompiler.dylib`, run under Rosetta. Nothing here is game
//! data except that a vertex shader's outputs repeat the pixel shader's
//! inputs.
//!
//! `VICTORY_HELPER_DXC` names the front end (default
//! `<out>/dxc/dxc-d3dmetal`, built on demand), `DXC_LIB` the library (default:
//! the one under `VIctory-macOS/target/wine-cx`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

use crate::sig::Element;

pub struct Dxc {
    helper: PathBuf,
    lib: PathBuf,
    dir: PathBuf,
    cache: Mutex<HashMap<String, Result<Vec<u8>, String>>>,
}

/// A pixel shader that writes one constant and reads nothing.
pub const DEFAULT_PS: &str =
    "float4 main() : SV_Target0 { return float4(0.25, 0.5, 0.75, 1.0); }\n";

impl Dxc {
    pub fn new(out: &Path) -> Result<Dxc, String> {
        let dir = out.join("dxc");
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let root = crate::project_root();
        let lib = std::env::var_os("DXC_LIB").map(PathBuf::from).unwrap_or_else(|| {
            root.join("VIctory-macOS/target/wine-cx/lib/external/D3DMetal.framework/Versions/A/Resources/libdxcompiler.dylib")
        });
        if !lib.exists() {
            return Err(format!("no DXC library at {} (set DXC_LIB)", lib.display()));
        }
        let helper = match std::env::var_os("VICTORY_HELPER_DXC") {
            Some(h) => PathBuf::from(h),
            None => {
                let h = dir.join("dxc-d3dmetal");
                if !h.exists() {
                    // The script lives in the checkout this binary was built from.
                    let script = Path::new(env!("CARGO_MANIFEST_DIR"))
                        .join("../../tools/macos/dxc-d3dmetal.sh");
                    let status = Command::new("/bin/sh")
                        .arg(script)
                        .arg(&h)
                        .status()
                        .map_err(|e| e.to_string())?;
                    if !status.success() {
                        return Err("building the DXC front end failed".into());
                    }
                }
                h
            }
        };
        Ok(Dxc {
            helper,
            lib,
            dir,
            cache: Mutex::new(HashMap::new()),
        })
    }

    /// DXIL of `hlsl` for `profile`; the same text is compiled once.
    pub fn compile(&self, profile: &str, hlsl: &str) -> Result<Vec<u8>, String> {
        let key = blake3::hash(format!("{profile}\n{hlsl}").as_bytes()).to_hex()[..16].to_owned();
        if let Some(r) = self
            .cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&key)
        {
            return r.clone();
        }
        let (src, out) = (
            self.dir.join(format!("{key}.hlsl")),
            self.dir.join(format!("{key}.dxil")),
        );
        let result = (|| {
            if let Ok(bytes) = std::fs::read(&out) {
                return Ok(bytes);
            }
            std::fs::write(&src, hlsl).map_err(|e| e.to_string())?;
            let run = Command::new("/usr/bin/arch")
                .arg("-x86_64")
                .arg(&self.helper)
                .args(["-T", profile, "-E", "main", "-Fo"])
                .arg(&out)
                .arg(&src)
                .env("DXC_LIB", &self.lib)
                .output()
                .map_err(|e| format!("run DXC: {e}"))?;
            if !run.status.success() {
                return Err(String::from_utf8_lossy(&run.stderr)
                    .lines()
                    .find(|l| l.contains("error"))
                    .unwrap_or("DXC failed")
                    .to_owned());
            }
            std::fs::read(&out).map_err(|e| e.to_string())
        })();
        self.cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key, result.clone());
        result
    }
}

fn comp_type(e: &Element) -> (&'static str, &'static str) {
    match e.comp {
        1 => ("uint4", "nointerpolation "),
        2 => ("int4", "nointerpolation "),
        _ => ("float4", ""),
    }
}

/// HLSL of a full-screen-triangle vertex shader that writes every ordinary
/// input the pixel shader reads (`inputs`: its `ISG1`), so that the pair
/// links. Elements are declared in register order, which is how DXC
/// allocates them again.
pub fn vertex_for(inputs: &[Element]) -> String {
    let mut els: Vec<&Element> = inputs.iter().filter(|e| e.system_value == 0).collect();
    els.sort_by_key(|e| (e.register, e.index));
    let mut s = String::from("struct O {\n  float4 pos : SV_Position;\n");
    for (i, e) in els.iter().enumerate() {
        let (ty, interp) = comp_type(e);
        s.push_str(&format!("  {interp}{ty} v{i} : {}{};\n", e.name, e.index));
    }
    s.push_str("};\nO main(uint id : SV_VertexID) {\n  O o;\n");
    s.push_str("  float2 uv = float2((id << 1) & 2, id & 2);\n  o.pos = float4(uv * 2.0 - 1.0, 0.0, 1.0);\n");
    for (i, e) in els.iter().enumerate() {
        let ty = comp_type(e).0;
        s.push_str(&format!("  o.v{i} = {ty}({}, 0, 0, 0);\n", i + 1));
    }
    s.push_str("  return o;\n}\n");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn el(name: &str, index: u32, sv: u32, comp: u32, register: u32) -> Element {
        Element {
            name: name.into(),
            index,
            system_value: sv,
            comp,
            register,
            mask: 15,
        }
    }

    #[test]
    fn the_vertex_shader_repeats_the_ordinary_inputs_in_register_order() {
        let hlsl = vertex_for(&[
            el("TEXCOORD", 1, 0, 3, 2),
            el("SV_Position", 0, 1, 3, 0),
            el("TEXCOORD", 0, 0, 1, 1),
        ]);
        let first = hlsl.find("TEXCOORD0").unwrap();
        let second = hlsl.find("TEXCOORD1").unwrap();
        assert!(first < second);
        assert!(hlsl.contains("nointerpolation uint4 v0 : TEXCOORD0"));
        assert!(hlsl.contains("float4 v1 : TEXCOORD1"));
        assert!(!hlsl.contains("SV_Position :") || hlsl.matches("SV_Position").count() == 1);
    }
}
