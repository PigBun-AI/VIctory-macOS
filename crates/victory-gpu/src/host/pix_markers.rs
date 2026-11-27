//! Legacy PIX Unicode payloads; no WinPixEventRuntime dependency.
use std::sync::{Mutex, OnceLock};
use windows::Win32::Graphics::Direct3D12::ID3D12GraphicsCommandList as List;

fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_PIX_MARKERS").is_some_and(|v| v == "1"))
}

// All labels are ASCII. Encode in a bounded stack buffer, including the NUL.
fn label<'a>(name: &str, mut n: u32, out: &'a mut [u16; 64]) -> &'a [u16] {
    let mut i = 0;
    for b in name.bytes().chain(b" #".iter().copied()) {
        out[i] = u16::from(b);
        i += 1;
    }
    let first = i;
    loop {
        out[i] = u16::from(b'0') + (n % 10) as u16;
        i += 1;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    out[first..i].reverse();
    out[i] = 0;
    &out[..=i]
}
fn begin(list: &List, name: &str, n: u32) {
    let mut buf = [0; 64];
    let text = label(name, n, &mut buf);
    unsafe {
        list.BeginEvent(
            0,
            Some(text.as_ptr().cast()),
            std::mem::size_of_val(text) as u32,
        )
    };
}
#[derive(Default)]
struct Stack {
    depth: usize,
    rt: bool,
    next: u32,
}
impl Stack {
    fn drain(&mut self, mut end: impl FnMut()) -> usize {
        let extra = self.depth.saturating_sub(1 + usize::from(self.rt));
        for _ in 0..self.depth {
            end();
        }
        *self = Self::default();
        extra
    }
}
pub(crate) struct Markers(Option<Mutex<Stack>>);
impl Markers {
    pub(super) fn new(list: &List) -> Self {
        let m = Self(enabled().then(Mutex::default));
        m.open(list);
        m
    }
    pub(super) fn open(&self, list: &List) {
        if let Some(s) = &self.0 {
            begin(list, "Direct/Main", 0);
            s.lock().unwrap().depth = 1;
        }
    }
    pub(crate) fn target(&self, list: &List) {
        if let Some(s) = &self.0 {
            let mut s = s.lock().unwrap();
            if s.rt {
                unsafe { list.EndEvent() };
            } else {
                s.depth += 1;
                s.rt = true;
            }
            s.next += 1;
            begin(list, "RT/DSV", s.next);
        }
    }
    pub(crate) fn dispatch(&self, list: &List, builder: impl FnOnce() -> bool, run: impl FnOnce()) {
        let Some(s) = &self.0 else {
            return run();
        };
        let mut s = s.lock().unwrap();
        s.next += 1;
        begin(
            list,
            if builder() {
                "BVH/Dispatch"
            } else {
                "Dispatch"
            },
            s.next,
        );
        s.depth += 1;
        run();
        unsafe { list.EndEvent() };
        s.depth -= 1;
    }
    pub(super) fn close(&self, list: &List) {
        if let Some(s) = &self.0 {
            let extra = s.lock().unwrap().drain(|| unsafe { list.EndEvent() });
            if extra != 0 {
                tracing::warn!(extra, "repaired unclosed GPU events");
            }
        }
    }
}
// Stack guard for auxiliary lists, including error exits before Close.
pub(super) struct Range<'a>(&'a List);
impl Drop for Range<'_> {
    fn drop(&mut self) {
        unsafe { self.0.EndEvent() };
    }
}
pub(super) fn auxiliary<'a>(list: &'a List, name: &str) -> Option<Range<'a>> {
    enabled().then(|| {
        begin(list, name, 0);
        Range(list)
    })
}
#[cfg(test)]
mod pix_markers_tests {
    use super::*;
    #[test]
    fn unicode_payload() {
        assert_eq!(label("RT", 42, &mut [0; 64]), &[82, 84, 32, 35, 52, 50, 0]);
    }
    #[test]
    fn nested_and_early_close() {
        for (depth, extra) in [(2, 0), (3, 1)] {
            let mut s = Stack {
                depth,
                rt: true,
                next: 1,
            };
            let mut ends = 0;
            assert_eq!(s.drain(|| ends += 1), extra);
            assert_eq!(ends, depth);
            assert_eq!(s.drain(|| panic!()), 0);
        }
    }
}
