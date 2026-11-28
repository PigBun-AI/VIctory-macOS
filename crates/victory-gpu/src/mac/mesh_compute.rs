//! Whether the translator attaches the compute form and the fetch vertex program to a mesh shader
//! (docs/mac/PATCHES.md, S5; `docs/metal/mesh-compute.md`).
//!
//! The Metal backend runs a mesh pipeline Metal cannot take (its mesh output over 32 KiB) as a compute pass and a
//! vertex draw, from the `VMCS` part of the mesh shader's container. D3DMetal has no use for the part and
//! gets none. On the Metal chain `tools/mac/run.sh` exports `VICTORY_METAL_OUT` (and `VICTORY_METAL_LOG`) to the
//! title: that is how the exe knows which chain it is on (neither the adapter nor the D3D12 surface says).
//!
//! A back that does not know the part would take it for nothing (the converter ignores it), but its denylists name
//! a stage by the hash of the whole container, which the part changes. So the default on the Metal chain is the
//! problem programs only when the back says it runs the part: `VICTORY_METAL_MESH_COMPUTE_SUPPORTED=1`, which
//! `run.sh` exports when the Metal stage's `CAPS` file has a line `mesh_compute` (written when the stage is made).
//! Without it the default is off.
//!
//! `VICTORY_MESH_COMPUTE=0|1|2` overrides the answer: 0 off, 1 only the programs whose Metal mesh struct is over
//! 32 KiB (the default on the Metal chain), 2 every mesh program that can be lowered.
//!
//! Hooked in by: `host::Host` creation (`set_host_mesh_compute`), which tells the translator before any shader is
//! translated; the shader cache keys on it through `host_settings`.

use victory_shader::emit::MeshComputeMode;

/// What the translator is told: [`MeshComputeMode::Problem`] on the Metal chain with a back that runs the part, off elsewhere.
pub fn host_mode() -> MeshComputeMode {
    decide(
        std::env::var("VICTORY_MESH_COMPUTE").ok().as_deref(),
        super::metal_chain::on_metal_chain(),
        std::env::var("VICTORY_METAL_MESH_COMPUTE_SUPPORTED").as_deref() == Ok("1"),
    )
}

fn decide(var: Option<&str>, metal_chain: bool, back_supports: bool) -> MeshComputeMode {
    match var {
        Some("0") => MeshComputeMode::Off,
        Some("1") => MeshComputeMode::Problem,
        Some("2") => MeshComputeMode::All,
        _ if metal_chain && back_supports => MeshComputeMode::Problem,
        _ => MeshComputeMode::Off,
    }
}

#[cfg(test)]
mod tests {
    use super::{MeshComputeMode::*, decide};

    #[test]
    fn the_metal_chain_gets_the_problem_programs_only_from_a_back_that_runs_the_part() {
        assert_eq!(decide(None, true, true), Problem);
        // an older back: the part would change the hash its denylists name a stage by
        assert_eq!(decide(None, true, false), Off);
        // D3DMetal has no use for it, whatever the variable says
        assert_eq!(decide(None, false, true), Off);
        assert_eq!(decide(None, false, false), Off);
        // an unknown value is no override
        assert_eq!(decide(Some("x"), true, true), Problem);
        assert_eq!(decide(Some("x"), true, false), Off);
        assert_eq!(decide(Some(""), false, false), Off);
    }

    #[test]
    fn the_variable_overrides_the_chain_and_the_back() {
        for chain in [false, true] {
            for back in [false, true] {
                assert_eq!(decide(Some("0"), chain, back), Off);
                assert_eq!(decide(Some("1"), chain, back), Problem);
                assert_eq!(decide(Some("2"), chain, back), All);
            }
        }
    }
}
