//! Typed buffer SRVs that start off the texture-buffer alignment.
//!
//! Metal wants a texture buffer at a multiple of 16 bytes
//! ([`crate::typed`]), and Metal Shader Converter 4.0.3 does not apply the
//! descriptor's "texture view offset in elements" to typed reads (measured
//! again 2026-11-27: the offset field, the typed flag and the bounds-check
//! compatibility flag change nothing; `GetDimensions` reports the texture's
//! width). D3D12 asks nothing of a typed view's `FirstElement`, and the game
//! makes `R16_UINT` views at odd element numbers (byte 167517650 of a title
//! memory copy).
//!
//! A view like that cannot be the same bytes through another `MTLBuffer`
//! (layering rule 6: a page reaches the GPU through one buffer). So its
//! descriptor points at a **mirror**: a private buffer of its own, aligned,
//! holding a copy of the viewed range, with the texture buffer over the
//! mirror. The mirror is refreshed by a blit from the source at the start of
//! every command list ([`CommandList::refresh_mirrors`], called by
//! `Queue::begin_list`), followed by one barrier from the copies to
//! everything. What that does and does not give:
//!
//! * the bytes of the source as of the start of the list: CPU writes made
//!   before the list was recorded, GPU writes of lists that ran before it
//!   (the queue orders submissions);
//! * **not** what the same list writes into the source before it reads the
//!   view (a dispatch that fills the range and a later one that reads it
//!   through the misaligned view sees the start-of-list bytes);
//! * only read views (SRV): a UAV through a mirror would write the copy, so
//!   a misaligned typed UAV stays refused ([`crate::typed`]).
//!
//! The three-channel 32-bit formats (`R32G32B32_*`) take the same road in every case, aligned
//! or not: the mirror is **expanded** to one four-channel element per three-channel one and
//! refreshed by a compute pass instead of a copy ([`crate::expand`], [`crate::typed`]).
//!
//! Limits: at most [`MAX_MIRRORS`] mirrors and [`MAX_MIRROR_BYTES`] bytes at
//! a time (each costs a blit per list for as long as its source lives); past
//! them the view is refused as before, with the reason. A mirror lives and
//! dies with its source buffer (retired after the same GPU work).

use std::sync::{Mutex, Weak};

use crate::error::{Error, Result};
use crate::memory::{Buffer, BufferInner, BufferKind};
use crate::stages::{ALL, BLIT, VIS_DEVICE};
use crate::typed::{self, TypedFormat};

/// Most mirrors alive at once, per device.
pub const MAX_MIRRORS: usize = 256;
/// Most bytes of mirrors alive at once, per device.
pub const MAX_MIRROR_BYTES: u64 = 1 << 30;

/// What makes a mirror different from another of the same buffer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct MirrorKey {
    pub(crate) pixel: usize,
    pub(crate) offset: u64,
    pub(crate) elements: u64,
    /// An expanded mirror of a three-channel format.
    pub(crate) expand: bool,
}

/// A copy of a range of a buffer, and the texture buffer over it.
pub(crate) struct Mirror {
    pub(crate) key: MirrorKey,
    pub(crate) buffer: Buffer,
    pub(crate) resource_id: u64,
    /// Bytes of the mirror (what the limits count).
    pub(crate) len: u64,
    /// Bytes read from the source each time the mirror is refreshed.
    pub(crate) src_len: u64,
    /// The value of the fourth channel of an expanded mirror (bits); `None` for a plain copy.
    pub(crate) fourth: Option<u32>,
}

/// The buffers of a device that have mirrors, and what the mirrors cost.
#[derive(Default)]
pub(crate) struct Registry {
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    buffers: Vec<Weak<BufferInner>>,
    count: usize,
    bytes: u64,
}

impl Registry {
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Room for one more mirror of `bytes`? Takes it when there is.
    fn reserve(&self, bytes: u64) -> bool {
        let mut s = self.lock();
        if s.count >= MAX_MIRRORS || s.bytes + bytes > MAX_MIRROR_BYTES {
            return false;
        }
        s.count += 1;
        s.bytes += bytes;
        true
    }

    fn note_buffer(&self, b: Weak<BufferInner>) {
        self.lock().buffers.push(b);
    }

    /// A buffer with `count` mirrors of `bytes` in all is gone.
    pub(crate) fn release(&self, count: usize, bytes: u64) {
        let mut s = self.lock();
        s.count = s.count.saturating_sub(count);
        s.bytes = s.bytes.saturating_sub(bytes);
    }

    /// The live buffers that have mirrors.
    fn live(&self) -> Vec<Buffer> {
        let mut s = self.lock();
        s.buffers.retain(|w| w.strong_count() > 0);
        s.buffers
            .iter()
            .filter_map(Weak::upgrade)
            .map(|inner| Buffer { inner })
            .collect()
    }
}

/// What a descriptor needs of a mirror.
#[derive(Clone, Copy, Debug)]
pub struct MirrorView {
    /// `gpuAddress` of the mirror buffer (the descriptor's address).
    pub gpu_va: u64,
    /// The texture buffer over it.
    pub resource_id: u64,
}

impl Buffer {
    /// A mirror of `elements` elements of `format` at byte `first_byte` of
    /// this buffer (see the module comment), made on first request and the
    /// same one after. Refused when it is outside the buffer
    /// ([`Error::BadArgument`]) or over the limits ([`Error::Unsupported`]).
    pub fn typed_mirror(
        &self,
        format: &TypedFormat,
        first_byte: u64,
        elements: u64,
    ) -> Result<MirrorView> {
        let bytes = u64::from(format.bytes);
        let src_len = elements.checked_mul(bytes);
        let end = src_len.and_then(|n| n.checked_add(first_byte));
        let (Some(src_len), Some(end)) = (src_len, end) else {
            return Err(Error::BadArgument("typed view range overflows".into()));
        };
        // What the mirror holds: the same bytes, or the expanded elements.
        let len = if format.three_channel {
            elements
                .checked_mul(u64::from(format.mirror_bytes()))
                .ok_or_else(|| Error::BadArgument("typed view range overflows".into()))?
        } else {
            src_len
        };
        if elements == 0 || end > self.len() {
            return Err(Error::BadArgument(format!(
                "a typed view of {elements} x {bytes} bytes at byte {first_byte} is outside a buffer of {} bytes",
                self.len()
            )));
        }
        let key = MirrorKey {
            pixel: format.pixel.0,
            offset: first_byte,
            elements,
            expand: format.three_channel,
        };
        let mut mirrors = self.inner.mirrors.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(m) = mirrors.iter().find(|m| m.key == key) {
            return Ok(MirrorView {
                gpu_va: m.buffer.gpu_va(),
                resource_id: m.resource_id,
            });
        }
        let registry = &self.inner.device.mirrors;
        if !registry.reserve(len) {
            return Err(Error::Unsupported(format!(
                "a typed view starting at byte {first_byte} is not on the {}-byte alignment of a Metal texture buffer, \
                 and {MAX_MIRRORS} mirrors or {MAX_MIRROR_BYTES} bytes of them already exist",
                typed::texture_alignment(&self.inner.device.raw, format.pixel)
            )));
        }
        let device = crate::device::Device {
            inner: self.inner.device.clone(),
        };
        // The texture over the mirror has the mirror's element size.
        let view_format = TypedFormat {
            bytes: format.mirror_bytes(),
            three_channel: false,
            ..*format
        };
        let made = device
            .create_buffer(BufferKind::Default, len.next_multiple_of(16))
            .and_then(|buffer| {
                let view =
                    buffer.texture_view(&view_format, 0, elements, typed::ViewAccess::Read)?;
                Ok((buffer, view.resource_id))
            });
        let (buffer, resource_id) = match made {
            Ok(m) => m,
            Err(e) => {
                registry.release(1, len);
                return Err(e);
            }
        };
        let view = MirrorView {
            gpu_va: buffer.gpu_va(),
            resource_id,
        };
        if mirrors.is_empty() {
            registry.note_buffer(std::sync::Arc::downgrade(&self.inner));
        }
        mirrors.push(Mirror {
            key,
            buffer,
            resource_id,
            len,
            src_len,
            fourth: format.three_channel.then_some(
                if format.dxgi == typed::dxgi::R32G32B32_FLOAT {
                    0x3f80_0000
                } else {
                    1
                },
            ),
        });
        Ok(view)
    }

    /// How many mirrors this buffer has.
    pub fn mirror_count(&self) -> usize {
        self.inner
            .mirrors
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .len()
    }

    /// The mirrors of this buffer: `(mirror, byte offset in the buffer, bytes read, fourth
    /// channel of an expanded one)`.
    fn mirror_copies(&self) -> Vec<(Buffer, u64, u64, Option<u32>)> {
        let mirrors = self.inner.mirrors.lock().unwrap_or_else(|e| e.into_inner());
        mirrors
            .iter()
            .map(|m| (m.buffer.clone(), m.key.offset, m.src_len, m.fourth))
            .collect()
    }
}

impl crate::record::CommandList {
    /// Copies the source of every live mirror into it (see the module
    /// comment); nothing is encoded when the device has none.
    pub(crate) fn refresh_mirrors(&mut self) -> Result<()> {
        let registry = &self.queue().device().inner.mirrors;
        let sources = registry.live();
        let (mut copied, mut expanded) = (false, false);
        for src in &sources {
            for (mirror, offset, len, fourth) in src.mirror_copies() {
                match fourth {
                    None => {
                        self.copy_buffer(&mirror, 0, src, offset, len)?;
                        copied = true;
                    }
                    Some(bits) => {
                        self.expand_three_channel(&mirror, src, offset, len / 12, bits)?;
                        expanded = true;
                    }
                }
            }
        }
        if copied {
            self.barrier(BLIT, ALL, VIS_DEVICE)?;
        }
        if expanded {
            self.barrier(crate::stages::DISPATCH, ALL, VIS_DEVICE)?;
        }
        Ok(())
    }
}

impl crate::device::Device {
    /// How many mirrors exist on this device and how many bytes they copy.
    pub fn mirror_totals(&self) -> (usize, u64) {
        let s = self.inner.mirrors.lock();
        (s.count, s.bytes)
    }
}
