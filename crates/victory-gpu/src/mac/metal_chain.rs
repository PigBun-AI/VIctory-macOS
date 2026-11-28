//! Whether the exe runs on our own Metal chain (docs/mac/PATCHES.md, S5 and S7), as opposed to D3DMetal or Windows.
//!
//! Neither the adapter nor the D3D12 surface says which chain the exe is on, so `tools/mac/run.sh` exports
//! `VICTORY_METAL_OUT` (the Metal stage) to the title on the Metal chain only. S5 (mesh as compute) and S7 (the native
//! 16-bit `fptrunc` of the nearest rounding) take their default from it; D3DMetal, the reference path, keeps the
//! translation as it was. "No `attributeAtVertex`" is not the test: D3DMetal has no `attributeAtVertex` either.
//!
//! Hooked in by: `host::Host` creation (`set_host_metal_chain`), which tells the translator before any shader is
//! translated; the shader cache keys on it through `host_settings` (bit 28).

/// True when `VICTORY_METAL_OUT` is set and not empty.
pub fn on_metal_chain() -> bool {
    decide(std::env::var_os("VICTORY_METAL_OUT").as_deref())
}

fn decide(out: Option<&std::ffi::OsStr>) -> bool {
    out.is_some_and(|v| !v.is_empty())
}

#[cfg(test)]
mod tests {
    use super::decide;
    use std::ffi::OsStr;

    #[test]
    fn only_a_stage_in_the_environment_makes_the_metal_chain() {
        assert!(decide(Some(OsStr::new("/stage"))));
        // D3DMetal and Windows: run.sh exports nothing, or an empty value
        assert!(!decide(None));
        assert!(!decide(Some(OsStr::new(""))));
    }
}
