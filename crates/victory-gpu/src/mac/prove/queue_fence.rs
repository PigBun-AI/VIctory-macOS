//! C-V-017 (docs/mac/PATCHES.md G12): queues, fences, allocators and the device-removed poll as the title calls them,
//! through the objects' method tables (the offsets the title calls at), against the D3D12 specification.
//!
//! Reference: D3D12 functional spec (the Direct3D 12 programming guide, "Fence-based resource management" and
//! "Synchronization"): `ID3D12Fence::GetCompletedValue` is the highest value signalled; `SetEventOnCompletion(v, e)` sets
//! `e` when the value reaches `v`, at once if it already has; `ID3D12CommandQueue::Signal` sets the fence after all work
//! submitted before it; `ID3D12CommandQueue::Wait(f, v)` holds back everything submitted to the queue after it until `f`
//! reaches `v` (a wait for a value already reached holds nothing); `ID3D12Device::GetDeviceRemovedReason` is S_OK on a
//! healthy device; `ID3D12CommandAllocator::Reset` and `ID3D12GraphicsCommandList::Reset` return S_OK.
use std::ffi::c_void;

use crate::object::{HRESULT, S_OK, This, slot};
use crate::queue::QueueDesc;

type Release = extern "system" fn(This) -> u32;
type Completed = extern "system" fn(This) -> u64;
type SetEvent = extern "system" fn(This, u64, *mut c_void) -> HRESULT;
type FenceSignal = extern "system" fn(This, u64) -> HRESULT;
type QueueSignal = extern "system" fn(This, This, u64) -> HRESULT;
type QueueWait = extern "system" fn(This, This, u64) -> HRESULT;

fn fence(initial: u64) -> This {
    let mut f = std::ptr::null();
    assert_eq!(crate::fence::create(std::ptr::null(), initial, 0, &crate::fence::FENCE_IIDS[0], &mut f), S_OK);
    f
}

fn queue(kind: u32) -> This {
    let desc = QueueDesc { kind, priority: 0, flags: 0, nodes: 0 };
    let mut q = std::ptr::null();
    assert_eq!(crate::queue::create(std::ptr::null(), &desc, &crate::queue::QUEUE_IIDS[0], &mut q), S_OK);
    q
}

fn signal_offset(q: This) -> usize {
    crate::queue::signal_offset_for_tests(q)
}

fn completed(f: This) -> u64 {
    slot::<Completed>(f, 0x40)(f)
}

#[test]
fn a_fence_holds_the_highest_value_signalled() {
    let f = fence(5);
    assert_eq!(completed(f), 5, "the initial value");
    assert_eq!(slot::<FenceSignal>(f, 0x50)(f, 9), S_OK);
    assert_eq!(completed(f), 9);
    // the title reads the value from memory at the fence's GPU address as well
    let at = slot::<Completed>(f, 0x60)(f);
    // SAFETY: the address of the fence's value, which lives as long as the fence.
    assert_eq!(unsafe { (at as *const u64).read() }, 9);
    assert_eq!(slot::<Release>(f, 0x10)(f), 0);
}

#[test]
fn a_queue_signal_sets_the_fence_and_signals_keep_their_order() {
    let q = queue(0);
    let f = fence(0);
    let signal = slot::<QueueSignal>(q, signal_offset(q));
    for v in [1, 2, 7] {
        assert_eq!(signal(q, f, v), S_OK);
        assert_eq!(completed(f), v);
    }
    assert_eq!(signal(q, std::ptr::null(), 8), crate::object::E_INVALIDARG);
    assert_eq!(completed(f), 7);
}

#[cfg(windows)]
mod events {
    use super::*;
    use windows_sys::Win32::System::Threading::{CreateEventW, WaitForSingleObject};

    fn event() -> *mut c_void {
        // SAFETY: a plain manual-reset event, unnamed.
        let e = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
        assert!(!e.is_null());
        e
    }
    fn set(e: *mut c_void) -> bool {
        // SAFETY: our event.
        unsafe { WaitForSingleObject(e, 0) == 0 }
    }

    #[test]
    fn an_event_is_set_when_the_fence_reaches_its_value_and_not_before() {
        let f = fence(2);
        let wait = slot::<SetEvent>(f, 0x48);
        let (reached, three, five, nine) = (event(), event(), event(), event());
        assert_eq!(wait(f, 2, reached), S_OK);
        assert!(set(reached), "a value already reached sets the event at once");
        for (e, v) in [(three, 3), (five, 5), (nine, 9)] {
            assert_eq!(wait(f, v, e), S_OK);
            assert!(!set(e), "value {v}: not before");
        }
        slot::<FenceSignal>(f, 0x50)(f, 3);
        assert!(set(three) && !set(five) && !set(nine), "3 sets only the event waiting for 3");
        slot::<FenceSignal>(f, 0x50)(f, 9);
        assert!(set(five) && set(nine), "9 sets every event waiting for 9 or less");
    }

    #[test]
    fn a_queue_wait_holds_what_comes_after_it_until_the_fence_gets_there() {
        let (a, b) = (queue(0), queue(2));
        let (f, g) = (fence(0), fence(0));
        let (a_signal, a_wait) = (slot::<QueueSignal>(a, signal_offset(a)), slot::<QueueWait>(a, signal_offset(a) + 8));
        let b_signal = slot::<QueueSignal>(b, signal_offset(b));
        // a waits for f = 1, then signals g = 1 (the signal is behind the wait)
        assert_eq!(a_wait(a, f, 1), S_OK);
        assert_eq!(a_signal(a, g, 1), S_OK);
        assert_eq!(completed(g), 0, "held behind the wait");
        // b reaches f = 1: a's queue goes on
        assert_eq!(b_signal(b, f, 1), S_OK);
        assert_eq!(completed(f), 1);
        assert_eq!(completed(g), 1, "released once the fence was there");
        // a wait for a value already reached holds nothing
        assert_eq!(a_wait(a, f, 1), S_OK);
        assert_eq!(a_signal(a, g, 2), S_OK);
        assert_eq!(completed(g), 2);
        // a chain: waits for 2 values from two queues, in order
        assert_eq!(a_wait(a, f, 5), S_OK);
        assert_eq!(a_signal(a, g, 3), S_OK);
        assert_eq!(completed(g), 2);
        assert_eq!(b_signal(b, f, 4), S_OK);
        assert_eq!(completed(g), 2, "4 is not 5");
        assert_eq!(b_signal(b, f, 5), S_OK);
        assert_eq!(completed(g), 3);
    }
}

#[test]
fn allocators_and_lists_reset_and_the_healthy_device_is_not_removed() {
    let null = std::ptr::null();
    let mut allocator = null;
    assert_eq!(crate::list::create_allocator(null, 0, &crate::list::ALLOCATOR_IIDS[0], &mut allocator), S_OK);
    assert_eq!(slot::<extern "system" fn(This) -> HRESULT>(allocator, 0x40)(allocator), S_OK);
    let device = crate::device::device();
    assert_eq!(slot::<extern "system" fn(This) -> HRESULT>(device, 0x128)(device), S_OK, "GetDeviceRemovedReason of a healthy device");
}
