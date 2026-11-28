use super::*;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use windows_sys::Win32::System::Threading::{CreateEventW, ResetEvent, WaitForSingleObject};
type P = *mut c_void;
type Signal = extern "system" fn(P, P, u64);
type Set = extern "system" fn(P, P);
type Ref = extern "system" fn(P) -> u32;
fn event(manual: bool) -> OwnedHandle {
    let h = unsafe { CreateEventW(std::ptr::null(), manual as i32, 0, std::ptr::null()) };
    assert!(!h.is_null());
    unsafe { OwnedHandle::from_raw_handle(h) }
}
fn waiting(h: &OwnedHandle, ms: u32) -> u32 {
    unsafe { WaitForSingleObject(h.as_raw_handle(), ms) }
}
fn gpu_slot<F: Copy>(p: P, slot: usize) -> F {
    let f = unsafe { *(*p.cast::<*const *const ()>()).add(slot) };
    assert_eq!(size_of::<F>(), size_of::<usize>());
    unsafe { std::mem::transmute_copy(&f) }
}
fn fence() -> P {
    use victory_gpu::{
        device,
        object::{Iid, guid},
    };
    let mut d = std::ptr::null();
    assert_eq!(
        device::create(
            std::ptr::null(),
            &device::CHECKED_VERSION,
            &device::IID,
            &mut d
        ),
        0
    );
    let d = d.cast_mut().cast();
    let create: extern "system" fn(P, u64, u32, *const Iid, Out) -> HRESULT = gpu_slot(d, 36);
    let mut f = std::ptr::null_mut();
    let iid = guid(
        0x0a753dcf,
        0xc4d8,
        0x4b91,
        [0xad, 0xf6, 0xbe, 0x5a, 0x60, 0xd9, 0x5a, 0x76],
    );
    assert_eq!(create(d, 0, 0, &iid, &mut f), 0);
    gpu_slot::<Ref>(d, 2)(d);
    f
}
#[test]
fn x2_factory_x1_queue_every_slot_and_waiters() {
    let q = new_queue(1);
    let a = status_array(1);
    let f = fence();
    let target = 0xfedc_ba98_0123_4567;
    let waiter = event(false);
    let manual = event(true);
    let auto = event(false);
    let qi: extern "system" fn(P, *const Guid, Out) -> HRESULT = call(q, 0);
    let mut out = q;
    assert_eq!(qi(q, &FACTORY, &mut out), E_NOINTERFACE);
    assert!(out.is_null());
    assert_eq!(qi(q, &abi::QIDS[0], &mut out), 0);
    assert_ne!(out, q); // exact base view; one canonical IUnknown
    let unk = Guid::new(0, 0, 0, [192, 0, 0, 0, 0, 0, 0, 70]);
    let base = out;
    assert_eq!(qi(q, &abi::QIDS[2], &mut out), S_OK);
    let x2 = out;
    call::<Set>(x2, 104)(x2, std::ptr::null_mut());
    assert_eq!(
        call::<extern "system" fn(P, *const Guid, Out) -> HRESULT>(x2, 0)(x2, &unk, &mut out),
        S_OK
    );
    assert_eq!(out, base);
    call::<Ref>(base, 16)(base);
    call::<Ref>(x2, 16)(x2);
    let rel: Ref = call(q, 16);
    assert_eq!(call::<Ref>(out, 16)(out), 1);
    let add: Ref = call(q, 8);
    assert_eq!(add(q), 2);
    assert_eq!(rel(q), 1);
    let source = [55u8; 4];
    let mut dest = [0u8; 4];
    enqueue(
        q,
        &request(
            MEMORY_SOURCE,
            dest.as_mut_ptr().cast(),
            4,
            source.as_ptr().cast(),
            0,
            4,
        ),
    );
    let arm: extern "system" fn(P, u64, P) -> HRESULT = gpu_slot(f, 9);
    assert_eq!(arm(f, target, waiter.as_raw_handle()), 0);
    let signal: Signal = call(q, 40);
    signal(q, f, target);
    let set: Set = call(q, 96);
    let copy = manual.try_clone().unwrap();
    set(q, copy.as_raw_handle());
    drop(copy);
    set(q, auto.as_raw_handle());
    enqueue_status(q, a, 0);
    assert_eq!(waiting(&waiter, 0), 258);
    assert_eq!(waiting(&manual, 0), 258);
    void(q, 48);
    assert_eq!(wait(a, 0), 0);
    assert_eq!(dest, source);
    let get: extern "system" fn(P) -> u64 = gpu_slot(f, 8);
    if std::env::var_os("VICTORY_DSTORAGE_NOTIFY").as_deref() == Some(std::ffi::OsStr::new("0")) {
        assert_eq!(get(f), 0);
        for h in [&waiter, &manual, &auto] {
            assert_eq!(waiting(h, 0), 258)
        }
    } else {
        assert_eq!(waiting(&waiter, 1000), 0);
        assert_eq!(get(f), target);
        assert_eq!(waiting(&manual, 0), 0);
        assert_eq!(waiting(&manual, 0), 0);
        assert_ne!(unsafe { ResetEvent(manual.as_raw_handle()) }, 0);
        assert_eq!(waiting(&manual, 0), 258);
        assert_eq!(waiting(&auto, 0), 0);
        assert_eq!(waiting(&auto, 0), 258);
    }
    let cancel: extern "system" fn(P, u64, u64) = call(q, 56);
    cancel(q, 0, 0);
    let err: extern "system" fn(P) -> P = call(q, 72);
    assert!(err(q).is_null());
    let mut record = [u64::MAX; 128];
    let retrieve: Set = call(q, 80);
    retrieve(q, record.as_mut_ptr().cast());
    assert_eq!(record[0] as u32, 0);
    let query: Set = call(q, 88);
    query(q, record.as_mut_ptr().cast());
    void(q, 64);
    assert_eq!(rel(q), 0);
    gpu_slot::<Ref>(f, 2)(f);
    call::<Ref>(a, 16)(a);
}

/// Run the actual CRT exit hook in an isolated process (not a render of fake counts).
#[test]
fn exit_snapshot_pairs_counts_and_unpublished_markers() {
    const CHILD: &str = "VICTORY_TEST_DSTORAGE_EXIT_CHILD";
    if std::env::var_os(CHILD).is_some() {
        let q = new_queue(1);
        let a = status_array(1);
        let e = event(true);
        let f = fence();
        call::<Set>(q, 96)(q, e.as_raw_handle());
        call::<Signal>(q, 40)(q, f, 0x1234_5678_9000_0000);
        assert_eq!(gpu_slot::<Ref>(f, 2)(f), 1); // the unsubmitted marker owns it
        enqueue_status(q, a, 0);
        let closed = new_queue(1);
        call::<Set>(closed, 96)(closed, e.as_raw_handle());
        void(closed, 64);
        call::<Ref>(closed, 16)(closed);
        let qi: extern "system" fn(P, *const Guid, Out) -> HRESULT = call(q, 0);
        let mut out = q;
        assert_eq!(qi(q, &FACTORY, &mut out), E_NOINTERFACE);
        assert!(out.is_null());
        victory_loader::mac::int3dump::STORAGE_SNAPSHOT
            .get()
            .unwrap()("L7-test");
        unsafe extern "C" {
            fn exit(code: i32) -> !;
        }
        unsafe { exit(0) }
    }
    let child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "exit_snapshot_pairs_counts_and_unpublished_markers",
            "--nocapture",
        ])
        .env(CHILD, "1")
        .env("VICTORY_DSTORAGE_NOTIFY", "1")
        .output()
        .unwrap();
    let s = String::from_utf8_lossy(&child.stderr);
    assert!(child.status.success(), "{s}");
    for reason in ["L7-test", "crt-exit"] {
        let body = s
            .split(&format!("dstorage.snapshot reason={reason}"))
            .nth(1)
            .unwrap()
            .split("dstorage.snapshot reason=")
            .next()
            .unwrap();
        for row in [
            "dstorage.summary iface=queueX1 slot=0 calls=1 rejected=1",
            "dstorage.hr.summary iface=queueX1 slot=0 hr=80004002 count=1",
            "dstorage.queue queue=0",
            "pending=3 oldest_seq=Some(0)",
            "dstorage.unpublished queue=0 seq=0 type=event enqueue_ms=",
            "dstorage.unpublished queue=0 seq=1 type=fence enqueue_ms=",
            "dstorage.unpublished queue=0 seq=2 type=status enqueue_ms=",
            "dstorage.unpublished queue=1 seq=3 type=event enqueue_ms=",
            "reason=cancelled-by-close",
        ] {
            assert!(body.contains(row), "missing {row}: {body}");
        }
    }
}

#[test]
fn b_then_a_retires_marker_before_c() {
    if std::env::var_os("VICTORY_DSTORAGE_NOTIFY").as_deref() == Some(std::ffi::OsStr::new("0")) {
        return;
    }
    let e = event(false);
    let marker = Command::External(External::event(e.try_clone().unwrap()));
    let mut l = lanes(STAGING);
    l.lanes.insert(0, Lane::new(0, 64));
    read(&mut l, 0, 0, 1);
    read(&mut l, 0, 0, 1);
    l.lanes.get_mut(&0).unwrap().unsubmitted.push(marker);
    read(&mut l, 0, 0, 1);
    l.submit(0, false);
    let a = l.start().unwrap();
    let b = l.start().unwrap();
    let c = l.start().unwrap();
    l.finish(&b, Ok(()));
    assert!(l.lanes.get_mut(&0).unwrap().take_external().is_none());
    assert_eq!(waiting(&e, 0), 258);
    l.finish(&a, Ok(()));
    let lane = l.lanes.get_mut(&0).unwrap();
    let (_, m) = lane.take_external().unwrap();
    assert!(lane.running.contains(&c.seq));
    assert!(m.publish());
    assert_eq!(waiting(&e, 0), 0);
    lane.publishing = None;
    l.finish(&c, Ok(()));
    assert!(l.lanes.get_mut(&0).unwrap().take_external().is_none());
}
