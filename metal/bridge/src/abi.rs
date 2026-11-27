//! Call codes and argument blocks that cross the PE/unixlib line.
//!
//! Every block is `repr(C)`, made of fixed-width integers, floats and byte
//! arrays, and has the same layout in the PE (x86-64 Windows ABI) and in the
//! Mach-O half (x86-64 or arm64 SysV ABI): that is the whole contract.
//! `Link` moves into `victory-metal-proto` with M1.

use std::ffi::c_void;

pub const OP_NOP: u32 = 0;
/// Args: [`Args256`]. The unixlib reads all 256 words.
pub const OP_ARGS256: u32 = 1;
/// Args: [`InitArgs`].
pub const OP_INIT: u32 = 2;
/// Args: [`EmptyCbArgs`].
pub const OP_EMPTY_CB: u32 = 3;
/// Args: [`WrapArgs`].
pub const OP_WRAP: u32 = 4;
/// Args: [`HandleArgs`].
pub const OP_RELEASE: u32 = 5;
/// Args: [`GpuArgs`].
pub const OP_GPU_SUM: u32 = 6;
/// Args: [`GpuArgs`].
pub const OP_GPU_XOR: u32 = 7;
/// Args: [`BwArgs`].
pub const OP_READ_BW: u32 = 8;
/// Args: [`ResidencyArgs`].
pub const OP_RESIDENCY: u32 = 9;
/// Args: [`LayerArgs`].
pub const OP_LAYER_ACQUIRE: u32 = 10;
/// Args: [`LayerArgs`].
pub const OP_LAYER_RELEASE: u32 = 11;
/// Args: [`PresentArgs`].
pub const OP_PRESENT_CLEAR: u32 = 12;
/// Number of call codes; the size of the unixlib table.
pub const OP_COUNT: usize = 13;

/// `NTSTATUS` values, as Wine returns them to the PE.
pub const STATUS_SUCCESS: i32 = 0;
pub const STATUS_UNSUCCESSFUL: i32 = 0xC000_0001_u32 as i32;
pub const STATUS_NOT_SUPPORTED: i32 = 0xC000_00BB_u32 as i32;
pub const STATUS_INVALID_PARAMETER: i32 = 0xC000_000D_u32 as i32;

/// `empty_cb` and `residency` queue kinds.
pub const KIND_CLASSIC: u32 = 0;
pub const KIND_METAL4: u32 = 1;

/// `ResidencyArgs::step`.
pub const RES_BUILD: u32 = 0;
pub const RES_STATE: u32 = 1;
pub const RES_USE: u32 = 2;

/// How a PE reaches the Metal side. `Link` for Wine is `pe::WineUnixlib`;
/// the in-process one is `unix::InProcess`.
pub trait Link {
    /// Calls `op` with a pointer to its argument block; returns an `NTSTATUS`.
    fn call(&self, op: u32, args: *mut c_void) -> i32;
}

#[repr(C)]
pub struct Args256 {
    pub v: [u32; 256],
}

#[repr(C)]
pub struct InitArgs {
    pub name: [u8; 96],
    pub max_buffer_len: u64,
    pub working_set: u64,
    pub unified_memory: u32,
    pub translated: u32,
    pub page_size: u32,
    pub metal4_family: u32,
    /// 1 when the Metal 4 queue, allocator, command buffer and event exist.
    pub metal4_ok: u32,
    pub _pad: u32,
    /// Why not, when `metal4_ok` is 0 (NUL-terminated).
    pub metal4_note: [u8; 128],
}

#[repr(C)]
pub struct EmptyCbArgs {
    pub kind: u32,
    pub iterations: u32,
    /// Out: total time spent inside the unixlib for all iterations.
    pub elapsed_ns: u64,
}

#[repr(C)]
pub struct WrapArgs {
    /// Address to wrap; 0 asks for a Metal-owned shared buffer instead.
    pub ptr: u64,
    pub len: u64,
    /// Out.
    pub handle: u32,
    pub _pad: u32,
}

#[repr(C)]
pub struct HandleArgs {
    pub handle: u32,
    pub _pad: u32,
}

#[repr(C)]
pub struct GpuArgs {
    pub handle: u32,
    pub words: u32,
    pub key: u32,
    /// Out for `OP_GPU_SUM`.
    pub result: u32,
}

#[repr(C)]
pub struct BwArgs {
    pub handle: u32,
    pub _pad: u32,
    pub len: u64,
    /// Out: GPU start to end, and commit to completion seen by the CPU.
    pub gpu_ns: u64,
    pub wall_ns: u64,
}

#[repr(C)]
pub struct ResidencyArgs {
    pub handle: u32,
    pub step: u32,
    pub kind: u32,
    pub words: u32,
    /// Out: GPU sum for `RES_USE` on the classic queue.
    pub result: u32,
    /// Out: `RES_STATE` says whether the set contains the buffer.
    pub flag: u32,
    /// Out: `RES_BUILD` create, add, commit, request; `RES_STATE` count.
    pub t: [u64; 4],
}

#[repr(C)]
pub struct LayerArgs {
    pub hwnd: u64,
    /// Out for acquire; in for release.
    pub layer: u64,
    pub view: u64,
    pub device: u64,
    /// Out: how far acquire got (see `unix::wine`), 5 = all the way.
    pub stage: u32,
    pub _pad: u32,
}

#[repr(C)]
pub struct PresentArgs {
    pub layer: u64,
    pub rgba: [f64; 4],
    /// Out.
    pub width: u32,
    pub height: u32,
    pub pixel_format: u32,
    pub pixel: u32,
    pub readback: u32,
    pub _pad: u32,
    pub error: [u8; 160],
}

/// Copies `text` into a NUL-terminated byte array, cutting it if needed.
pub fn put_text(dst: &mut [u8], text: &str) {
    let n = text.len().min(dst.len() - 1);
    dst[..n].copy_from_slice(&text.as_bytes()[..n]);
    dst[n..].fill(0);
}

/// The text in a NUL-terminated byte array.
pub fn get_text(src: &[u8]) -> String {
    let end = src.iter().position(|&b| b == 0).unwrap_or(src.len());
    String::from_utf8_lossy(&src[..end]).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::{offset_of, size_of};

    /// The two halves are built separately; these sizes are the contract.
    #[test]
    fn layouts_are_fixed() {
        assert_eq!(size_of::<Args256>(), 1024);
        assert_eq!(size_of::<InitArgs>(), 96 + 16 + 24 + 128);
        assert_eq!(size_of::<EmptyCbArgs>(), 16);
        assert_eq!(size_of::<WrapArgs>(), 24);
        assert_eq!(size_of::<GpuArgs>(), 16);
        assert_eq!(size_of::<BwArgs>(), 32);
        assert_eq!(size_of::<ResidencyArgs>(), 24 + 32);
        assert_eq!(size_of::<LayerArgs>(), 40);
        assert_eq!(size_of::<PresentArgs>(), 8 + 32 + 24 + 160);
        assert_eq!(offset_of!(WrapArgs, handle), 16);
        assert_eq!(offset_of!(BwArgs, gpu_ns), 16);
        assert_eq!(offset_of!(PresentArgs, error), 8 + 32 + 24);
    }

    #[test]
    fn text_round_trips_and_truncates() {
        let mut buf = [0xffu8; 8];
        put_text(&mut buf, "abc");
        assert_eq!(get_text(&buf), "abc");
        put_text(&mut buf, "0123456789");
        assert_eq!(get_text(&buf), "0123456");
    }
}
