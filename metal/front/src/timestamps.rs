//! Things a command list does that need no new back object: timestamp query
//! heaps, `WriteBufferImmediate`, PIX markers.
//!
//! **Timestamps are an approximation, on purpose** (known difference from a
//! GPU's clock, see docs/metal/m4-newbase.md). `EndQuery(TIMESTAMP, i)` stores
//! a strictly increasing CPU tick (nanoseconds, the unit
//! `GetTimestampFrequency` reports) in the heap *at recording time*, and
//! `ResolveQueryData` writes those stored values into the destination buffer
//! with `WRITE_IMMEDIATE`. So the values rise in the order the queries were
//! recorded and differences are record-to-record CPU time, not GPU time. The
//! exact path exists in the Metal 4 SDK (counter heaps and timestamp
//! writes; the basis is written down in docs/metal/m4-newbase.md, since the
//! front names no Metal) and replaces this when something needs real GPU
//! durations.
//!
//! PIX markers become `DEBUG_LABEL` commands (the back drops them for now):
//! `BeginEvent` → `[name`, `SetMarker` → `=name`, `EndEvent` → `]`.

use std::sync::Mutex;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use victory_metal_proto::cmd::{Command, MAX_LABEL, WriteImmediate};

use crate::device::{Buffer, CommandList, Item, ListState};
use crate::hresult::Hr;

/// `D3D12_QUERY_HEAP_TYPE_TIMESTAMP`.
pub const HEAP_TYPE_TIMESTAMP: u32 = 1;
/// `D3D12_QUERY_HEAP_TYPE_COPY_QUEUE_TIMESTAMP`.
pub const HEAP_TYPE_COPY_QUEUE_TIMESTAMP: u32 = 5;
/// `D3D12_QUERY_TYPE_TIMESTAMP`.
pub const QUERY_TYPE_TIMESTAMP: u32 = 2;

/// Ticks per second of [`now`] (what `GetTimestampFrequency` answers).
pub const FREQUENCY: u64 = 1_000_000_000;

/// A strictly increasing tick count: nanoseconds since the first call, plus a
/// second so that no stamp is zero.
pub fn now() -> u64 {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    static LAST: AtomicU64 = AtomicU64::new(0);
    let ns = EPOCH.get_or_init(Instant::now).elapsed().as_nanos() as u64 + FREQUENCY;
    let mut last = LAST.load(Ordering::Relaxed);
    loop {
        let next = ns.max(last + 1);
        match LAST.compare_exchange_weak(last, next, Ordering::AcqRel, Ordering::Relaxed) {
            Ok(_) => return next,
            Err(seen) => last = seen,
        }
    }
}

/// A timestamp query heap: the stamp of each slot, taken when `EndQuery` is recorded.
pub struct QueryHeap {
    pub ty: u32,
    pub count: u32,
    stamps: Mutex<Vec<u64>>,
}

impl QueryHeap {
    /// Only timestamp heaps exist; other types are `E_NOTIMPL` for the caller to say.
    pub fn new(ty: u32, count: u32) -> Result<QueryHeap, Hr> {
        if !matches!(ty, HEAP_TYPE_TIMESTAMP | HEAP_TYPE_COPY_QUEUE_TIMESTAMP) {
            return Err(Hr::E_NOTIMPL);
        }
        if count == 0 {
            return Err(Hr::E_INVALIDARG);
        }
        Ok(QueryHeap {
            ty,
            count,
            stamps: Mutex::new(vec![0; count as usize]),
        })
    }

    /// Records the stamp of slot `index`.
    pub fn stamp(&self, index: u32) -> Result<(), Hr> {
        let mut s = self.stamps.lock().unwrap();
        *s.get_mut(index as usize).ok_or(Hr::E_INVALIDARG)? = now();
        Ok(())
    }

    /// The stamp of slot `index` (0 = never ended).
    pub fn get(&self, index: u32) -> Option<u64> {
        self.stamps.lock().unwrap().get(index as usize).copied()
    }
}

/// The text of a PIX marker (`metadata` 0 = UTF-16, 1 = ANSI, as
/// `PIX_EVENT_UNICODE_VERSION` / `PIX_EVENT_ANSI_VERSION`; the packed 3.x
/// blob is not decoded). Cut at the first NUL and at [`MAX_LABEL`] bytes
/// (on a character boundary).
pub fn pix_text(metadata: u32, data: &[u8]) -> Option<String> {
    let mut text = match metadata {
        0 => {
            let units: Vec<u16> = data
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| u16::from_le_bytes(*c))
                .take_while(|&u| u != 0)
                .collect();
            String::from_utf16_lossy(&units)
        }
        1 => {
            let end = data.iter().position(|&b| b == 0).unwrap_or(data.len());
            String::from_utf8_lossy(&data[..end]).into_owned()
        }
        _ => return None,
    };
    let mut max = MAX_LABEL - 1;
    if text.len() > max {
        while !text.is_char_boundary(max) {
            max -= 1;
        }
        text.truncate(max);
    }
    Some(text)
}

impl CommandList {
    /// `BeginEvent` / `SetMarker` / `EndEvent` as a label (see the module doc).
    pub fn marker(&mut self, prefix: char, text: &str) {
        let mut l = Vec::with_capacity(1 + text.len());
        let mut buf = [0u8; 4];
        l.extend_from_slice(prefix.encode_utf8(&mut buf).as_bytes());
        l.extend_from_slice(text.as_bytes());
        self.push_item(Item::Label(l));
    }

    /// `WriteBufferImmediate` of one parameter: `value` at GPU address `va`.
    /// `mode` is `D3D12_WRITEBUFFERIMMEDIATE_MODE` (0 default, 1 marker in, 2 marker out).
    pub fn write_immediate(&mut self, va: u64, value: u32, mode: u32) {
        if !va.is_multiple_of(4) || mode > WriteImmediate::MARKER_OUT {
            self.fail(Hr::E_INVALIDARG);
            return;
        }
        let Some((res, offset)) = self.device().va.find(va, 4) else {
            self.fail(Hr::E_INVALIDARG);
            return;
        };
        // The back writes through a blit: not inside a render pass.
        if self.state == ListState::Recording {
            self.leave_render();
        }
        self.push_item(Item::Cmd(Command::WriteImmediate(WriteImmediate {
            dst: victory_metal_proto::ResId(res),
            size: 4,
            offset,
            value: u64::from(value),
            mode,
            _pad: 0,
        })));
    }

    /// `EndQuery(TIMESTAMP, index)`.
    pub fn end_timestamp(&mut self, heap: &QueryHeap, index: u32) {
        if heap.stamp(index).is_err() {
            self.fail(Hr::E_INVALIDARG);
        }
    }

    /// `ResolveQueryData(heap, TIMESTAMP, start, count, dst, offset)`: the
    /// stamps taken so far, 8 bytes each, written in order.
    pub fn resolve_timestamps(
        &mut self,
        heap: &QueryHeap,
        start: u32,
        count: u32,
        dst: &Buffer,
        offset: u64,
    ) {
        let end = u64::from(start) + u64::from(count);
        let bytes = u64::from(count) * 8;
        if end > u64::from(heap.count)
            || !offset.is_multiple_of(8)
            || offset.checked_add(bytes).is_none_or(|e| e > dst.desc.width)
            || !dst.desc.is_buffer()
        {
            self.fail(Hr::E_INVALIDARG);
            return;
        }
        if self.state == ListState::Recording {
            self.leave_render();
        }
        for i in 0..count {
            self.push_item(Item::Cmd(Command::WriteImmediate(WriteImmediate {
                dst: dst.res,
                size: 8,
                offset: offset + u64::from(i) * 8,
                value: heap.get(start + i).unwrap_or(0),
                mode: WriteImmediate::DEFAULT,
                _pad: 0,
            })));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticks_only_rise() {
        let a = now();
        let b = now();
        assert!(b > a && a >= FREQUENCY);
    }

    #[test]
    fn pix_text_reads_both_encodings_and_stops_at_nul() {
        let w: Vec<u8> = "Pass 1\0junk"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        assert_eq!(pix_text(0, &w).as_deref(), Some("Pass 1"));
        assert_eq!(pix_text(1, b"abc\0def").as_deref(), Some("abc"));
        assert_eq!(pix_text(2, b"blob"), None);
        let long = "é".repeat(2000);
        let t = pix_text(1, long.as_bytes()).unwrap();
        assert!(t.len() < MAX_LABEL && t.chars().all(|c| c == 'é'));
    }

    #[test]
    fn query_heaps_are_timestamp_heaps() {
        assert_eq!(QueryHeap::new(0, 4).err(), Some(Hr::E_NOTIMPL));
        assert_eq!(
            QueryHeap::new(HEAP_TYPE_TIMESTAMP, 0).err(),
            Some(Hr::E_INVALIDARG)
        );
        let h = QueryHeap::new(HEAP_TYPE_TIMESTAMP, 2).unwrap();
        assert_eq!(h.get(0), Some(0));
        h.stamp(0).unwrap();
        h.stamp(1).unwrap();
        assert!(h.get(1) > h.get(0) && h.get(0) > Some(0));
        assert_eq!(h.stamp(2), Err(Hr::E_INVALIDARG));
    }
}
