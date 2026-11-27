//! Whether the host's shader compiler takes `dx.op.attributeAtVertex` (docs/mac/PATCHES.md, S1).
//!
//! The Windows line's host does. D3DMetal (the exe on Wine on a Mac) does not: its Metal Shader Converter
//! refuses the instruction. Where it does not, the translator gives a pixel shader the three vertices of
//! an attribute as flat inputs and the mesh stage fills them (`victory_shader::emit::vertex_flat`).
//!
//! `VICTORY_ATTRIBUTE_AT_VERTEX=0|1` overrides the answer (a Windows host can run the Mac form).
//!
//! Hooked in by: `host::Host` creation (`set_host_attribute_at_vertex`), which tells the translator
//! before any shader is translated; the shader cache keys on it through `host_settings`.

/// The capability: true on Windows, false on D3DMetal.
pub fn host_has_it() -> bool {
    decide(std::env::var("VICTORY_ATTRIBUTE_AT_VERTEX").ok().as_deref(), !super::rosetta::rosetta())
}

fn decide(var: Option<&str>, default: bool) -> bool {
    match var {
        Some("1") => true,
        Some("0") => false,
        _ => default,
    }
}

#[cfg(test)]
mod tests {
    use super::decide;

    #[test]
    fn the_variable_overrides_the_host() {
        assert!(decide(Some("1"), false));
        assert!(!decide(Some("0"), true));
        assert!(decide(None, true));
        assert!(!decide(None, false));
        assert!(decide(Some("x"), true));
    }
}
