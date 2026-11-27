//! Mac patch: stand-ins for the few `cfg(windows)` modules that cross-platform
//! code of the `feat/live-translator` line reaches by path (`crate::host`,
//! `crate::texture`, `crate::shader_cache`), so the crate also builds and its
//! portable tests run natively on macOS/Linux. None of this is compiled for
//! Windows (the real modules are there); see docs/mac/PATCHES.md, patch M1.
//!
//! Each stand-in has only what the cross-platform callers use:
//! * `list.rs` (`WaitCheck::holds`): `texture::committed`, `host::read_into`;
//! * `live.rs` (the live-toggles watcher and its tests): `shader_cache::counts`
//!   and `shader_cache::translation_of`.

pub(crate) mod texture {
    /// No host paging on a non-Windows build: memory counts as committed.
    pub(crate) fn committed(_at: *const u8, _len: usize) -> bool {
        true
    }
}

pub(crate) mod host {
    /// Reads `dst.len()` bytes at `src`; the Windows version asks the kernel
    /// and reports unreadable memory as `false`. Here the caller's address is
    /// ordinary readable memory (tests only).
    pub(crate) fn read_into(src: *const u8, dst: &mut [u8]) -> bool {
        // SAFETY: only reached from tests that wait on addresses they own.
        unsafe { std::ptr::copy_nonoverlapping(src, dst.as_mut_ptr(), dst.len()) };
        true
    }
}

pub(crate) mod shader_cache {
    use std::sync::Arc;

    use victory_shader::emit::live;

    /// Hits and misses of the shader cache: there is none off Windows.
    pub(crate) fn counts() -> (u64, u64) {
        (0, 0)
    }

    /// The translation key of a snapshot of the live switches, from the
    /// switches' values only (the real one also folds in the translator and
    /// the card): all `live.rs`'s tests need is that it follows them.
    pub(crate) fn translation_of(snap: &Arc<live::Snapshot>) -> [u8; 32] {
        use std::hash::{Hash, Hasher};
        let mut out = [0u8; 32];
        for (i, chunk) in out.chunks_mut(8).enumerate() {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            (i, snap.settings()).hash(&mut h);
            chunk.copy_from_slice(&h.finish().to_le_bytes());
        }
        out
    }
}
