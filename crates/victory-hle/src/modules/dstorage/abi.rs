//! Count only guest dispatch, never internal references or status reads.
use super::*;
const I: Guid = Guid::new(0, 0, 0, [192, 0, 0, 0, 0, 0, 0, 70]);
pub(super) const QIDS: [Guid; 3] = [
    Guid::new(
        0xa3f6649a,
        0x8c2c,
        0x4b3a,
        [0x8b, 0x6e, 0x10, 0x18, 0x6a, 0x04, 0xb9, 0xbc],
    ),
    Guid::new(
        0x3c02cb0c,
        0xc050,
        0x4195,
        [0x99, 0xa1, 0xa1, 0xce, 0x94, 0xba, 0x33, 0xc5],
    ),
    Guid::new(
        0x3af5c948,
        0x6297,
        0x426c,
        [0xbf, 0xfc, 0x8f, 0x18, 0x95, 0x9f, 0xb8, 0x7b],
    ),
];
pub(super) const ARRAY: Guid = Guid::new(
    0xcd66d752,
    0x2786,
    0x44f7,
    [0x90, 0x70, 0xe2, 0xa6, 0xfd, 0xad, 0x3f, 0x1b],
);
pub(super) const FILE: Guid = Guid::new(
    0x43704f9f,
    0x27c6,
    0x4a9f,
    [0x88, 0x15, 0xd1, 0x65, 0x27, 0x89, 0x4d, 0xf4],
);
const NAMES: [&str; 6] = [
    "factoryX2",
    "queueX",
    "queueX1",
    "queueX2",
    "status",
    "file",
];
const SLOTS: [usize; 6] = [11, 12, 13, 14, 5, 5];
struct Counts {
    calls: AtomicU64,
    rejected: AtomicU64,
    ok: AtomicU64,
    pending: AtomicU64,
}
static COUNTS: [[Counts; 14]; 6] = [const {
    [const {
        Counts {
            calls: AtomicU64::new(0),
            rejected: AtomicU64::new(0),
            ok: AtomicU64::new(0),
            pending: AtomicU64::new(0),
        }
    }; 14]
}; 6];
static ERR: Mutex<BTreeMap<(usize, usize, HRESULT), u64>> = Mutex::new(BTreeMap::new());
static START: std::sync::Once = std::sync::Once::new();
pub(super) fn init() {
    START.call_once(|| {
        let _ = victory_loader::mac::int3dump::STORAGE_SNAPSHOT.set(super::snapshot);
        unsafe extern "C" {
            fn atexit(f: extern "C" fn()) -> i32;
        }
        extern "C" fn end() {
            super::snapshot("crt-exit");
        }
        unsafe { atexit(end) };
    });
}
fn line(i: usize, s: usize, k: &str, n: u64) {
    if n.is_power_of_two() {
        eprintln!("dstorage.method iface={} slot={} {}={}", NAMES[i], s, k, n);
    }
}
fn bump(i: usize, s: usize, k: &str, c: &AtomicU64) -> u64 {
    let n = c.fetch_add(1, Ordering::Relaxed) + 1;
    line(i, s, k, n);
    n
}
fn hit(i: usize, s: usize) {
    bump(i, s, "calls", &COUNTS[i][s].calls);
}
pub(super) fn reject(i: usize, s: usize) {
    bump(i, s, "rejected", &COUNTS[i][s].rejected);
}
fn hr(i: usize, s: usize, r: HRESULT) -> HRESULT {
    let c = &COUNTS[i][s];
    match r {
        S_OK => {
            bump(i, s, "hr_00000000", &c.ok);
        }
        E_PENDING => {
            bump(i, s, "hr_8000000a", &c.pending);
        }
        _ => {
            let mut m = ERR.lock().unwrap_or_else(|e| e.into_inner());
            let n = m.entry((i, s, r)).or_default();
            *n += 1;
            if n.is_power_of_two() {
                eprintln!(
                    "dstorage.hr iface={} slot={s} hr={:08x} count={n}",
                    NAMES[i], r as u32
                );
            }
        }
    }
    r
}
fn summary(i: usize, s: usize) -> String {
    let c = &COUNTS[i][s];
    let get = |a: &AtomicU64| a.load(Ordering::Relaxed);
    format!(
        "dstorage.summary iface={} slot={s} calls={} rejected={} hr_00000000={} hr_8000000a={}",
        NAMES[i],
        get(&c.calls),
        get(&c.rejected),
        get(&c.ok),
        get(&c.pending)
    )
}
pub(super) fn dump() {
    if !START.is_completed() {
        return;
    }
    for (i, max) in SLOTS.iter().enumerate() {
        for s in 0..*max {
            eprintln!("{}", summary(i, s));
        }
    }
    if let Ok(m) = ERR.try_lock() {
        for (&(i, s, r), n) in m.iter() {
            eprintln!(
                "dstorage.hr.summary iface={} slot={s} hr={:08x} count={n}",
                NAMES[i], r as u32
            );
        }
    }
}
fn stub(i: usize, s: usize) {
    hit(i, s);
    if bump(i, s, "rejected", &COUNTS[i][s].rejected) == 1 {
        tracing::warn!(
            iface = NAMES[i],
            slot = s,
            "dstorage: method not implemented"
        );
    }
}
type P = *mut c_void;
extern "system" fn refs<const K: usize, const S: usize>(p: P) -> u32 {
    hit(K, S);
    refop::<K, S>(p)
}
fn refop<const K: usize, const S: usize>(p: P) -> u32 {
    macro_rules! op {
        ($t:ty) => {
            if S == 1 {
                add_ref_instance::<$t>(p.cast())
            } else {
                release::<$t>(p.cast())
            }
        };
    }
    match K {
        0 => 1,
        4 => op!(StatusArray),
        5 => op!(File),
        _ => QueueGroup::reference(p.cast(), S == 1),
    }
}
extern "system" fn qi<const K: usize>(p: P, id: *const Guid, out: Out) -> HRESULT {
    hit(K, 0);
    let id = unsafe { id.as_ref() };
    let target = match K {
        1..=3 => {
            let version = if id == Some(&I) {
                Some(0)
            } else {
                QIDS.iter().position(|q| Some(q) == id)
            };
            version.map(|i| QueueGroup::view(p.cast(), i))
        }
        _ => (id == Some(&I)
            || id
                == Some(match K {
                    0 => &FACTORY,
                    4 => &ARRAY,
                    _ => &FILE,
                }))
        .then_some(p),
    };
    let r = if out.is_null() {
        E_INVALIDARG
    } else if let Some(target) = target {
        refop::<K, 1>(p);
        put(out, target)
    } else {
        put(out, std::ptr::null_mut());
        E_NOINTERFACE
    };
    if r < 0 {
        reject(K, 0)
    }
    hr(K, 0, r)
}
macro_rules! factory {($name:ident,$s:literal,$f:ident,$($arg:ident:$t:ty),*)=>{extern "system" fn $name(p:Factory,$($arg:$t),*)->HRESULT{hit(0,$s);let r=$f(p,$($arg),*);if r<0{reject(0,$s)}hr(0,$s,r)}}}
factory!(open,4,open_file,path:*const u16,id:*const Guid,out:Out);
factory!(array,5,create_status_array,n:u32,name:*const c_char,id:*const Guid,out:Out);
factory!(staging,8,set_staging_buffer_size,n:u32);
factory!(create,10,create_queue,d:*const QueueDesc,id:*const Guid,out:Out);
extern "system" fn old(_: P, _: *const c_void, _: *const Guid, out: Out) -> HRESULT {
    stub(0, 3);
    put(out, std::ptr::null_mut());
    hr(0, 3, E_NOTIMPL)
}
extern "system" fn affinity(_: P, _: u64) -> HRESULT {
    stub(0, 6);
    hr(0, 6, E_NOTIMPL)
}
extern "system" fn flags(_: P, _: u32) {
    stub(0, 7)
}
extern "system" fn ptr_stub<const K: usize, const S: usize>(_: P, _: P) {
    stub(K, S)
}
extern "system" fn handle<const K: usize, const S: usize>(_: P) -> P {
    stub(K, S);
    std::ptr::null_mut()
}
extern "system" fn fclose(_: P) {
    stub(5, 3)
}
extern "system" fn poll(p: Array, n: u32) -> bool {
    hit(4, 3);
    if n as usize >= unsafe { &*p }.state.entries.len() {
        reject(4, 3)
    }
    is_complete(p, n)
}
extern "system" fn result(p: Array, n: u32) -> HRESULT {
    hit(4, 4);
    if n as usize >= unsafe { &*p }.state.entries.len() {
        reject(4, 4)
    }
    hr(4, 4, status_result(p, n))
}
extern "system" fn request<const K: usize>(p: QueueRef, r: *const Request) {
    hit(K, 3);
    enqueue_request(p, r)
}
extern "system" fn status<const K: usize>(p: QueueRef, a: Array, n: u32) {
    hit(K, 4);
    enqueue_status(p, a, n)
}
extern "system" fn signal<const K: usize>(p: QueueRef, f: P, v: u64) {
    hit(K, 5);
    super::enqueue_signal(p, f, v)
}
extern "system" fn send<const K: usize>(p: QueueRef) {
    hit(K, 6);
    submit(p)
}
extern "system" fn cancel<const K: usize>(_: P, _: u64, _: u64) {
    stub(K, 7)
}
extern "system" fn end<const K: usize>(p: QueueRef) {
    hit(K, 8);
    close(p)
}
extern "system" fn record<const K: usize>(_: P, r: *mut u32) {
    stub(K, 10);
    put(r, 0);
}
extern "system" fn event<const K: usize>(p: QueueRef, h: P) {
    hit(K, 12);
    super::enqueue_event(p, h)
}
macro_rules! table{($($f:expr),*)=>{Methods([$($f as *const ()),*])}}
pub(super) static FACTORY_VTABLE: Methods<11> = table!(
    qi::<0>,
    refs::<0, 1>,
    refs::<0, 2>,
    old,
    open,
    array,
    affinity,
    flags,
    staging,
    ptr_stub::<0, 9>,
    create
);
pub(super) static FILE_VTABLE: Methods<5> =
    table!(qi::<5>, refs::<5, 1>, refs::<5, 2>, fclose, handle::<5, 4>);
pub(super) static STATUS_VTABLE: Methods<5> =
    table!(qi::<4>, refs::<4, 1>, refs::<4, 2>, poll, result);
macro_rules! qt{($k:literal $(,$f:expr)*)=>{table!(qi::<$k>,refs::<$k,1>,refs::<$k,2>,request::<$k>,status::<$k>,signal::<$k>,send::<$k>,cancel::<$k>,end::<$k>,handle::<$k,9>,record::<$k>,ptr_stub::<$k,11> $(,$f)*)}}
static X: Methods<12> = qt!(1);
static X1: Methods<13> = qt!(2, event::<2>);
static X2: Methods<14> = qt!(3, event::<3>, ptr_stub::<3, 13>);
pub(super) fn queue_object(k: usize) -> Object {
    match k {
        1 => Object::new("queueX", &X),
        2 => Object::new("queueX1", &X1),
        _ => Object::new("queueX2", &X2),
    }
}
#[cfg(test)]
#[test]
fn summary_fields() {
    let s = summary(2, 12);
    assert!(s.starts_with("dstorage.summary iface=queueX1 slot=12 calls="));
    for k in [" rejected=", " hr_00000000=", " hr_8000000a="] {
        assert!(s.contains(k));
    }
}
