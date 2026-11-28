//! `VICTORY_METAL_ZERO_INIT=1`: memory the back hands out starts as zeros.
//!
//! A committed resource of the original API (and, on the console, a freshly
//! committed page) is zero filled. Metal promises nothing for a new
//! `MTLBuffer`, `MTLTexture` or `MTLHeap`: the pages may hold what an earlier
//! resource left. A title that reads a history texture or a buffer before it
//! writes it (temporal effects) then reads garbage. This switch zeroes
//! committed buffers (CPU memset for shared ones, a GPU fill for private
//! ones), committed textures (copies from a zero buffer, every mip and slice;
//! depth/stencil and multisample textures are left alone) and placement heaps
//! (a buffer over the whole heap, filled). It is a diagnostic: it costs a
//! queue submission and a wait per resource.

use crate::copy::{BufLoc, TexLoc};
use crate::device::Device;
use crate::error::Result;
use crate::format;
use crate::heap::Heap;
use crate::memory::{Buffer, BufferKind};
use crate::queue::Queue;
use crate::texture::{TexDim, Texture};

/// Whether the switch is on (read once).
pub fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_METAL_ZERO_INIT").is_some_and(|v| v != "0"))
}

fn log(what: &str) {
    if std::env::var_os("VICTORY_METAL_LOG").is_some_and(|v| v != "0") {
        eprintln!("victory-metal back: zero-init: {what}");
    }
}

fn run(
    device: &Device,
    f: impl FnOnce(&mut crate::record::CommandList) -> Result<()>,
) -> Result<()> {
    let q = Queue::new(device)?;
    let mut l = q.begin_list()?;
    f(&mut l)?;
    l.end()?;
    let v = q.submit(vec![l])?;
    q.wait_for(v, 30_000)
}

/// Zeroes a new committed buffer.
pub fn buffer(device: &Device, b: &Buffer) {
    let len = b.len();
    let r = match b.kind() {
        BufferKind::Default => run(device, |l| l.fill_buffer(b, 0, len, 0)),
        BufferKind::Wrapped => return,
        _ => {
            let p = b.cpu_addr() as *mut u8;
            if !p.is_null() {
                // SAFETY: the buffer's CPU mapping is `len` bytes long and nobody else has it yet.
                unsafe { std::ptr::write_bytes(p, 0, len as usize) };
            }
            Ok(())
        }
    };
    if let Err(e) = r {
        log(&format!("buffer of {len} bytes: {e}"));
    }
}

/// Zeroes a new placement heap through a buffer over all of it.
pub fn heap(device: &Device, h: &Heap) {
    let size = h.size();
    let r = h
        .create_buffer(device, 0, size)
        .and_then(|b| run(device, |l| l.fill_buffer(&b, 0, size, 0)));
    if let Err(e) = r {
        log(&format!("heap of {size} bytes: {e}"));
    }
}

/// Zeroes a new committed texture: every mip and slice, from a zero buffer.
pub fn texture(device: &Device, t: &Texture) {
    let d = *t.desc();
    let Ok(f) = format::require(d.format) else {
        return;
    };
    if f.has_depth_stencil() || d.samples > 1 {
        log(&format!(
            "{:?} left alone (depth/stencil or multisample)",
            d.format
        ));
        return;
    }
    let r = (|| -> Result<()> {
        let mut need = 0u64;
        let layout = |mip: u32| {
            let s = d.mip_size(mip);
            let pitch = f.row_bytes(s[0]).next_multiple_of(256);
            (s, pitch, u64::from(f.block_rows(s[1])))
        };
        for mip in 0..d.mips {
            let (s, pitch, rows) = layout(mip);
            need = need.max(pitch * rows * u64::from(s[2]));
        }
        let zeros = device.create_buffer(BufferKind::Shared, need.max(256))?;
        buffer(device, &zeros);
        run(device, |l| {
            for mip in 0..d.mips {
                let (s, pitch, rows) = layout(mip);
                for slice in 0..d.array_len() {
                    let dst = TexLoc {
                        texture: t,
                        mip,
                        slice,
                        plane: 0,
                        origin: [0, 0, 0],
                    };
                    let src = BufLoc {
                        buffer: &zeros,
                        offset: 0,
                        format: d.format,
                        row_pitch: pitch as u32,
                        rows: rows as u32,
                    };
                    let _ = TexDim::D2;
                    l.copy_buffer_to_texture(&dst, &src, s)?;
                }
            }
            Ok(())
        })
    })();
    if let Err(e) = r {
        log(&format!("texture {d:?}: {e}"));
    }
}
