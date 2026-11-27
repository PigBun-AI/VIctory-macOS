//! Mach-O half: what Wine calls, and the same thing without Wine.

use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, RwLock};

use victory_metal_proto::{Link, Status, SyncOp};

use victory_metal_back::m0::{self, Kind};

use crate::abi::*;

/// What Wine calls for each code: `NTSTATUS (*)(void *args)`.
pub type UnixEntry = unsafe extern "C" fn(*mut c_void) -> i32;

/// Wine looks this symbol up in the unixlib it loads next to the PE
/// (`get_builtin_unix_funcs` in `dlls/ntdll/unix/virtual.c`) and indexes it
/// with the call code. There is no length: `OP_COUNT` entries exactly.
#[allow(non_upper_case_globals)]
#[unsafe(no_mangle)]
pub static __wine_unix_call_funcs: [UnixEntry; OP_COUNT] = [
    nop,
    args256,
    init,
    empty_cb,
    wrap,
    release,
    gpu_sum,
    gpu_xor,
    read_bw,
    residency,
    wine::layer_acquire,
    wine::layer_release,
    present_clear,
    proto_submit,
    proto_call,
];

/// Calling the table directly, for native tests and the native baseline.
pub struct InProcess;

impl RawLink for InProcess {
    #[inline(always)]
    fn call(&self, op: u32, args: *mut c_void) -> i32 {
        match __wine_unix_call_funcs.get(op as usize) {
            // SAFETY: the caller passes the block the code documents.
            Some(entry) => unsafe { entry(args) },
            None => STATUS_INVALID_PARAMETER,
        }
    }
}

impl Link for InProcess {
    fn submit(&self, block: &[u8]) -> Status {
        executor().submit(block)
    }

    fn call(&self, op: SyncOp, args: *mut u8, len: usize) -> Status {
        executor().call(op, args, len)
    }
}

// ---- the proto entry points ------------------------------------------------

/// Where the proto goes on the Mach-O side: the back's interpreter, as a
/// proto `Link`. Nothing is installed at first; the back (or a test) calls
/// [`set_executor`] once at load, and may replace it.
static EXECUTOR: RwLock<Option<Arc<dyn Link>>> = RwLock::new(None);

/// Installs the executor that `OP_PROTO_SUBMIT` / `OP_PROTO_CALL` (and
/// [`InProcess`] as a proto `Link`) hand their blocks to. Returns the previous one.
pub fn set_executor(executor: Arc<dyn Link>) -> Option<Arc<dyn Link>> {
    EXECUTOR.write().unwrap().replace(executor)
}

/// Removes the executor: the proto calls answer `NOT_INITIALIZED` again.
pub fn clear_executor() -> Option<Arc<dyn Link>> {
    EXECUTOR.write().unwrap().take()
}

/// What answers while no executor is installed: the transport works (`NOP`),
/// the device does not.
struct NoExecutor;

impl Link for NoExecutor {
    fn submit(&self, _block: &[u8]) -> Status {
        Status::NOT_INITIALIZED
    }

    fn call(&self, op: SyncOp, _args: *mut u8, _len: usize) -> Status {
        if op == SyncOp::NOP {
            Status::OK
        } else {
            Status::NOT_INITIALIZED
        }
    }
}

fn executor() -> Arc<dyn Link> {
    match &*EXECUTOR.read().unwrap() {
        Some(e) => e.clone(),
        None => Arc::new(NoExecutor),
    }
}

unsafe extern "C" fn proto_submit(args: *mut c_void) -> i32 {
    if args.is_null() {
        return STATUS_INVALID_PARAMETER;
    }
    // SAFETY: the PE passes a `SubmitArgs`.
    let a = unsafe { &*args.cast::<SubmitArgs>() };
    if a.block == 0 && a.len != 0 {
        return status_to_nt(Status::INVALID_ARG);
    }
    let block: &[u8] = if a.len == 0 {
        &[]
    } else {
        // SAFETY: the PE keeps `len` bytes readable at `block` for the call.
        unsafe { std::slice::from_raw_parts(a.block as *const u8, a.len as usize) }
    };
    status_to_nt(executor().submit(block))
}

unsafe extern "C" fn proto_call(args: *mut c_void) -> i32 {
    if args.is_null() {
        return STATUS_INVALID_PARAMETER;
    }
    // SAFETY: the PE passes a `CallArgs`.
    let a = unsafe { &*args.cast::<CallArgs>() };
    status_to_nt(executor().call(SyncOp(a.op), a.args as *mut u8, a.len as usize))
}

fn status(ok: bool) -> i32 {
    if ok {
        STATUS_SUCCESS
    } else {
        STATUS_UNSUCCESSFUL
    }
}

fn kind(raw: u32) -> Option<Kind> {
    match raw {
        KIND_CLASSIC => Some(Kind::Classic),
        KIND_METAL4 => Some(Kind::Metal4),
        _ => None,
    }
}

unsafe extern "C" fn nop(_args: *mut c_void) -> i32 {
    STATUS_SUCCESS
}

static SINK: AtomicU32 = AtomicU32::new(0);

unsafe extern "C" fn args256(args: *mut c_void) -> i32 {
    // SAFETY: the PE passes an `Args256`.
    let args = unsafe { &*args.cast::<Args256>() };
    let mut sum = 0u32;
    for word in &args.v {
        // Volatile: the point is to pay for reading the block.
        sum = sum.wrapping_add(unsafe { std::ptr::read_volatile(word) });
    }
    SINK.store(sum, Ordering::Relaxed);
    STATUS_SUCCESS
}

unsafe extern "C" fn init(args: *mut c_void) -> i32 {
    // SAFETY: the PE passes an `InitArgs`.
    let args = unsafe { &mut *args.cast::<InitArgs>() };
    match m0::init() {
        Ok(info) => {
            put_text(&mut args.name, &info.name);
            args.max_buffer_len = info.max_buffer_len;
            args.working_set = info.working_set;
            args.unified_memory = info.unified_memory as u32;
            args.translated = info.translated as u32;
            args.page_size = info.page_size;
            args.metal4_family = info.metal4_family as u32;
            let metal4 = m0::init_metal4();
            args.metal4_ok = metal4.is_ok() as u32;
            put_text(&mut args.metal4_note, &metal4.err().unwrap_or_default());
            STATUS_SUCCESS
        }
        Err(_) => STATUS_UNSUCCESSFUL,
    }
}

unsafe extern "C" fn empty_cb(args: *mut c_void) -> i32 {
    // SAFETY: the PE passes an `EmptyCbArgs`.
    let args = unsafe { &mut *args.cast::<EmptyCbArgs>() };
    let Some(kind) = kind(args.kind) else {
        return STATUS_INVALID_PARAMETER;
    };
    match m0::empty_cb(kind, args.iterations) {
        Some(ns) => {
            args.elapsed_ns = ns;
            STATUS_SUCCESS
        }
        None => STATUS_UNSUCCESSFUL,
    }
}

unsafe extern "C" fn wrap(args: *mut c_void) -> i32 {
    // SAFETY: the PE passes a `WrapArgs`.
    let args = unsafe { &mut *args.cast::<WrapArgs>() };
    let handle = if args.ptr == 0 {
        m0::alloc(args.len as usize)
    } else {
        // SAFETY: the PE keeps the memory mapped until `OP_RELEASE`.
        unsafe { m0::wrap(args.ptr as *mut u8, args.len as usize) }
    };
    match handle {
        Some(handle) => {
            args.handle = handle;
            STATUS_SUCCESS
        }
        None => STATUS_UNSUCCESSFUL,
    }
}

unsafe extern "C" fn release(args: *mut c_void) -> i32 {
    // SAFETY: the PE passes a `HandleArgs`.
    let args = unsafe { &*args.cast::<HandleArgs>() };
    status(m0::release(args.handle))
}

unsafe extern "C" fn gpu_sum(args: *mut c_void) -> i32 {
    // SAFETY: the PE passes a `GpuArgs`.
    let args = unsafe { &mut *args.cast::<GpuArgs>() };
    match m0::sum(args.handle, args.words) {
        Some(sum) => {
            args.result = sum;
            STATUS_SUCCESS
        }
        None => STATUS_UNSUCCESSFUL,
    }
}

unsafe extern "C" fn gpu_xor(args: *mut c_void) -> i32 {
    // SAFETY: the PE passes a `GpuArgs`.
    let args = unsafe { &*args.cast::<GpuArgs>() };
    status(m0::xor(args.handle, args.words, args.key))
}

unsafe extern "C" fn read_bw(args: *mut c_void) -> i32 {
    // SAFETY: the PE passes a `BwArgs`.
    let args = unsafe { &mut *args.cast::<BwArgs>() };
    match m0::read_bw(args.handle, args.len as usize) {
        Some((gpu, wall)) => {
            args.gpu_ns = gpu;
            args.wall_ns = wall;
            STATUS_SUCCESS
        }
        None => STATUS_UNSUCCESSFUL,
    }
}

unsafe extern "C" fn residency(args: *mut c_void) -> i32 {
    // SAFETY: the PE passes a `ResidencyArgs`.
    let args = unsafe { &mut *args.cast::<ResidencyArgs>() };
    match args.step {
        RES_BUILD => match m0::residency_build(args.handle) {
            Some(t) => {
                args.t = [t.create_ns, t.add_ns, t.commit_ns, t.request_ns];
                STATUS_SUCCESS
            }
            None => STATUS_UNSUCCESSFUL,
        },
        RES_STATE => match m0::residency_state(args.handle) {
            Some((contains, count)) => {
                args.flag = contains as u32;
                args.t[0] = count;
                STATUS_SUCCESS
            }
            None => STATUS_UNSUCCESSFUL,
        },
        RES_USE => {
            let Some(kind) = kind(args.kind) else {
                return STATUS_INVALID_PARAMETER;
            };
            match m0::residency_use(args.handle, kind, args.words) {
                Some(result) => {
                    args.result = result;
                    STATUS_SUCCESS
                }
                None => STATUS_UNSUCCESSFUL,
            }
        }
        _ => STATUS_INVALID_PARAMETER,
    }
}

unsafe extern "C" fn present_clear(args: *mut c_void) -> i32 {
    // SAFETY: the PE passes a `PresentArgs`.
    let args = unsafe { &mut *args.cast::<PresentArgs>() };
    // SAFETY: the layer came from `OP_LAYER_ACQUIRE` and is still held.
    match unsafe { m0::present_clear(args.layer as *mut c_void, args.rgba) } {
        Ok(p) => {
            args.width = p.drawable_width;
            args.height = p.drawable_height;
            args.pixel_format = p.pixel_format;
            args.pixel = p.pixel;
            args.readback = p.readback as u32;
            STATUS_SUCCESS
        }
        Err(text) => {
            put_text(&mut args.error, &text);
            STATUS_UNSUCCESSFUL
        }
    }
}

/// Getting the Metal layer of a Wine window: the `SurfaceProvider` of
/// the architecture, Wine flavour.
///
/// winemac.drv exports `macdrv_functions`, the table it hands to D3DMetal
/// (`dlls/winemac.drv/d3dmetal.c`). Its entries, in order, are
/// `init_display_devices`, `get_win_data`, `release_win_data`,
/// `get_cocoa_window`, `create_metal_device`, `release_metal_device`,
/// `view_create_metal_view`, `view_get_metal_layer`,
/// `view_release_metal_view`, `on_main_thread`, then Win32 helpers we do
/// not use. D3DMetal calls them in the order get_win_data, create device,
/// create view, get layer, release_win_data; so do we.
pub mod wine {
    use std::ffi::{CStr, c_char, c_void};

    use super::*;

    const SLOT_GET_WIN_DATA: usize = 1;
    const SLOT_RELEASE_WIN_DATA: usize = 2;
    const SLOT_CREATE_DEVICE: usize = 4;
    const SLOT_RELEASE_DEVICE: usize = 5;
    const SLOT_CREATE_VIEW: usize = 6;
    const SLOT_GET_LAYER: usize = 7;
    const SLOT_RELEASE_VIEW: usize = 8;
    /// Offset of `client_cocoa_view` in `struct d3dmetal_macdrv_win_data`
    /// (hwnd, cocoa_window, cocoa_view, then it).
    const CLIENT_VIEW_OFFSET: usize = 24;

    /// Stages `acquire` can stop at; the number reached is reported back.
    pub const STAGE_NO_TABLE: u32 = 0;
    pub const STAGE_TABLE: u32 = 1;
    pub const STAGE_WIN_DATA: u32 = 2;
    pub const STAGE_DEVICE: u32 = 3;
    pub const STAGE_VIEW: u32 = 4;
    pub const STAGE_LAYER: u32 = 5;

    // dyld's image list; libc marks these deprecated in favour of a crate
    // that wraps the same two functions.
    unsafe extern "C" {
        fn _dyld_image_count() -> u32;
        fn _dyld_get_image_name(index: u32) -> *const c_char;
    }

    type F1 = unsafe extern "C" fn(*mut c_void) -> *mut c_void;
    type F2 = unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void;

    /// `macdrv_functions` from the winemac.so Wine has already loaded.
    fn table() -> Option<*const usize> {
        // SAFETY: dyld queries and a `RTLD_NOLOAD` open take no ownership.
        unsafe {
            for i in 0.._dyld_image_count() {
                let name = _dyld_get_image_name(i);
                if name.is_null() {
                    continue;
                }
                let path: &CStr = CStr::from_ptr(name as *const c_char);
                if !path.to_bytes().ends_with(b"/winemac.so") {
                    continue;
                }
                let handle = libc::dlopen(name, libc::RTLD_LAZY | libc::RTLD_NOLOAD);
                if handle.is_null() {
                    continue;
                }
                let symbol = libc::dlsym(handle, c"macdrv_functions".as_ptr());
                libc::dlclose(handle);
                if !symbol.is_null() {
                    return Some(symbol as *const usize);
                }
            }
        }
        None
    }

    fn slot(table: *const usize, index: usize) -> *const c_void {
        // SAFETY: the table is a struct of at least ten pointers.
        unsafe { *table.add(index) as *const c_void }
    }

    /// Fills in the layer, view and device of the window in `args.hwnd`.
    ///
    /// # Safety
    /// `args` points to a `LayerArgs`; `hwnd` is a window of this process.
    pub unsafe extern "C" fn layer_acquire(args: *mut c_void) -> i32 {
        // SAFETY: the PE passes a `LayerArgs`.
        let args = unsafe { &mut *args.cast::<LayerArgs>() };
        args.stage = STAGE_NO_TABLE;
        let Some(table) = table() else {
            return STATUS_NOT_SUPPORTED;
        };
        args.stage = STAGE_TABLE;
        // SAFETY: these are the functions winemac.drv put in its table, with
        // the signatures `d3dmetal.c` gives them.
        unsafe {
            let get_win_data: F1 = std::mem::transmute(slot(table, SLOT_GET_WIN_DATA));
            let release_win_data: unsafe extern "C" fn(*mut c_void) =
                std::mem::transmute(slot(table, SLOT_RELEASE_WIN_DATA));
            let create_device: unsafe extern "C" fn() -> *mut c_void =
                std::mem::transmute(slot(table, SLOT_CREATE_DEVICE));
            let release_device: unsafe extern "C" fn(*mut c_void) =
                std::mem::transmute(slot(table, SLOT_RELEASE_DEVICE));
            let create_view: F2 = std::mem::transmute(slot(table, SLOT_CREATE_VIEW));
            let get_layer: F1 = std::mem::transmute(slot(table, SLOT_GET_LAYER));
            let release_view: unsafe extern "C" fn(*mut c_void) =
                std::mem::transmute(slot(table, SLOT_RELEASE_VIEW));

            let data = get_win_data(args.hwnd as *mut c_void);
            if data.is_null() {
                return STATUS_UNSUCCESSFUL;
            }
            args.stage = STAGE_WIN_DATA;
            let client_view = *data
                .cast::<u8>()
                .add(CLIENT_VIEW_OFFSET)
                .cast::<*mut c_void>();
            let device = create_device();
            if device.is_null() {
                release_win_data(data);
                return STATUS_UNSUCCESSFUL;
            }
            args.stage = STAGE_DEVICE;
            let view = create_view(client_view, device);
            if view.is_null() {
                release_device(device);
                release_win_data(data);
                return STATUS_UNSUCCESSFUL;
            }
            args.stage = STAGE_VIEW;
            let layer = get_layer(view);
            release_win_data(data);
            if layer.is_null() {
                release_view(view);
                release_device(device);
                return STATUS_UNSUCCESSFUL;
            }
            args.stage = STAGE_LAYER;
            args.layer = layer as u64;
            args.view = view as u64;
            args.device = device as u64;
        }
        STATUS_SUCCESS
    }

    /// Gives back what `layer_acquire` made.
    ///
    /// # Safety
    /// `args` points to the `LayerArgs` that `layer_acquire` filled in.
    pub unsafe extern "C" fn layer_release(args: *mut c_void) -> i32 {
        // SAFETY: the PE passes the `LayerArgs` acquire filled in.
        let args = unsafe { &*args.cast::<LayerArgs>() };
        let Some(table) = table() else {
            return STATUS_NOT_SUPPORTED;
        };
        // SAFETY: as in `layer_acquire`.
        unsafe {
            let release_device: unsafe extern "C" fn(*mut c_void) =
                std::mem::transmute(slot(table, SLOT_RELEASE_DEVICE));
            let release_view: unsafe extern "C" fn(*mut c_void) =
                std::mem::transmute(slot(table, SLOT_RELEASE_VIEW));
            release_view(args.view as *mut c_void);
            release_device(args.device as *mut c_void);
        }
        STATUS_SUCCESS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_has_one_entry_per_code() {
        assert_eq!(__wine_unix_call_funcs.len(), OP_COUNT);
        assert_eq!(
            RawLink::call(&InProcess, OP_COUNT as u32, std::ptr::null_mut()),
            STATUS_INVALID_PARAMETER
        );
    }

    #[test]
    fn nop_and_args_calls_succeed() {
        assert_eq!(
            RawLink::call(&InProcess, OP_NOP, std::ptr::null_mut()),
            STATUS_SUCCESS
        );
        let mut args = Args256 { v: [1; 256] };
        assert_eq!(
            RawLink::call(&InProcess, OP_ARGS256, (&raw mut args).cast()),
            STATUS_SUCCESS
        );
        assert_eq!(SINK.load(Ordering::Relaxed), 256);
    }

    #[test]
    fn layer_needs_wine() {
        let mut args = LayerArgs {
            hwnd: 1,
            layer: 0,
            view: 0,
            device: 0,
            stage: 9,
            _pad: 0,
        };
        assert_eq!(
            RawLink::call(&InProcess, OP_LAYER_ACQUIRE, (&raw mut args).cast()),
            STATUS_NOT_SUPPORTED
        );
        assert_eq!(args.stage, wine::STAGE_NO_TABLE);
    }
}

#[cfg(test)]
mod proto_tests {
    use std::sync::Mutex;

    use super::*;

    /// The executor is process-global; tests that install one take this first.
    static SERIAL: Mutex<()> = Mutex::new(());

    #[derive(Default)]
    struct Recorder {
        blocks: Mutex<Vec<Vec<u8>>>,
        calls: Mutex<Vec<(u32, usize, u8)>>,
    }

    impl Link for Recorder {
        fn submit(&self, block: &[u8]) -> Status {
            self.blocks.lock().unwrap().push(block.to_vec());
            if block.first() == Some(&0xEE) {
                Status::BAD_STREAM
            } else {
                Status::OK
            }
        }

        fn call(&self, op: SyncOp, args: *mut u8, len: usize) -> Status {
            // SAFETY: the tests pass `len` writable bytes.
            let first = unsafe { *args };
            self.calls.lock().unwrap().push((op.0, len, first));
            // Output: the back writes into the block it was given.
            unsafe { *args = 0x5a };
            if op == SyncOp::HELLO {
                Status::DEVICE_LOST
            } else {
                Status::OK
            }
        }
    }

    #[test]
    fn without_an_executor_the_transport_works_and_the_device_does_not() {
        let _g = SERIAL.lock().unwrap();
        clear_executor();
        let mut b = [0u8; 8];
        assert_eq!(
            Link::call(&InProcess, SyncOp::NOP, b.as_mut_ptr(), 8),
            Status::OK
        );
        assert_eq!(
            Link::call(&InProcess, SyncOp::HELLO, b.as_mut_ptr(), 8),
            Status::NOT_INITIALIZED
        );
        assert_eq!(
            Link::submit(&InProcess, &[1, 2, 3]),
            Status::NOT_INITIALIZED
        );
    }

    #[test]
    fn blocks_reach_the_installed_executor_through_the_table_entries() {
        let _g = SERIAL.lock().unwrap();
        let rec = Arc::new(Recorder::default());
        set_executor(rec.clone());

        // The way Wine calls: the table entry with an argument pointer.
        let block = [7u8, 8, 9];
        let mut s = SubmitArgs {
            block: block.as_ptr() as u64,
            len: 3,
        };
        let nt =
            unsafe { __wine_unix_call_funcs[OP_PROTO_SUBMIT as usize](&raw mut s as *mut c_void) };
        assert_eq!(nt_to_status(nt), Status::OK);
        let bad = [0xEEu8];
        s = SubmitArgs {
            block: bad.as_ptr() as u64,
            len: 1,
        };
        let nt =
            unsafe { __wine_unix_call_funcs[OP_PROTO_SUBMIT as usize](&raw mut s as *mut c_void) };
        assert_eq!(nt_to_status(nt), Status::BAD_STREAM);
        assert!(nt < 0);

        let mut buf = [1u8; 16];
        let mut c = CallArgs {
            op: SyncOp::QUERY_CAPS.0,
            _pad: 0,
            args: buf.as_mut_ptr() as u64,
            len: 16,
        };
        let nt =
            unsafe { __wine_unix_call_funcs[OP_PROTO_CALL as usize](&raw mut c as *mut c_void) };
        assert_eq!(nt_to_status(nt), Status::OK);
        assert_eq!(buf[0], 0x5a, "outputs are written into the caller's block");
        c.op = SyncOp::HELLO.0;
        let nt =
            unsafe { __wine_unix_call_funcs[OP_PROTO_CALL as usize](&raw mut c as *mut c_void) };
        assert_eq!(nt_to_status(nt), Status::DEVICE_LOST);

        // And in process, as a proto Link.
        assert_eq!(Link::submit(&InProcess, &[4]), Status::OK);
        assert_eq!(
            *rec.blocks.lock().unwrap(),
            vec![vec![7, 8, 9], vec![0xEE], vec![4]]
        );
        assert_eq!(rec.calls.lock().unwrap()[0], (SyncOp::QUERY_CAPS.0, 16, 1));
        clear_executor();
    }

    #[test]
    fn bad_pointers_are_refused_not_followed() {
        let _g = SERIAL.lock().unwrap();
        clear_executor();
        assert_eq!(
            unsafe { proto_submit(std::ptr::null_mut()) },
            STATUS_INVALID_PARAMETER
        );
        assert_eq!(
            unsafe { proto_call(std::ptr::null_mut()) },
            STATUS_INVALID_PARAMETER
        );
        let mut s = SubmitArgs { block: 0, len: 5 };
        let nt = unsafe { proto_submit(&raw mut s as *mut c_void) };
        assert_eq!(nt_to_status(nt), Status::INVALID_ARG);
    }
}
