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
/// proto `Link`.
enum Slot {
    /// Nothing asked for yet: the first proto call installs the back's own
    /// `Executor` (see [`executor`]).
    Default,
    /// [`set_executor`] put this one here.
    Installed(Arc<dyn Link>),
    /// [`clear_executor`]: no executor; the proto calls answer `NOT_INITIALIZED`.
    Cleared,
}

static EXECUTOR: RwLock<Slot> = RwLock::new(Slot::Default);

/// Installs the executor that `OP_PROTO_SUBMIT` / `OP_PROTO_CALL` (and
/// [`InProcess`] as a proto `Link`) hand their blocks to, in place of the
/// back's own one. Returns the previous one, if one was installed.
pub fn set_executor(executor: Arc<dyn Link>) -> Option<Arc<dyn Link>> {
    match std::mem::replace(&mut *EXECUTOR.write().unwrap(), Slot::Installed(executor)) {
        Slot::Installed(old) => Some(old),
        _ => None,
    }
}

/// Removes the executor: the proto calls answer `NOT_INITIALIZED` until
/// [`set_executor`] or [`use_default_executor`].
pub fn clear_executor() -> Option<Arc<dyn Link>> {
    match std::mem::replace(&mut *EXECUTOR.write().unwrap(), Slot::Cleared) {
        Slot::Installed(old) => Some(old),
        _ => None,
    }
}

/// Goes back to the back's own interpreter (a fresh `Executor`, made at the
/// next proto call). This is the state at load time: the Wine unixlib needs
/// no setup call; the first `OP_PROTO_CALL` (the front's `HELLO`) creates the
/// `Executor`, which makes its Metal device only when `HELLO` arrives.
pub fn use_default_executor() -> Option<Arc<dyn Link>> {
    match std::mem::replace(&mut *EXECUTOR.write().unwrap(), Slot::Default) {
        Slot::Installed(old) => Some(old),
        _ => None,
    }
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

/// The bridge is the one place that knows both the Wine entry points and the
/// back: the back never sees Wine, the front never sees the back.
fn executor() -> Arc<dyn Link> {
    match &*EXECUTOR.read().unwrap() {
        Slot::Installed(e) => return e.clone(),
        Slot::Cleared => return Arc::new(NoExecutor),
        Slot::Default => {}
    }
    let mut slot = EXECUTOR.write().unwrap();
    match &*slot {
        Slot::Installed(e) => e.clone(),
        Slot::Cleared => Arc::new(NoExecutor),
        Slot::Default => {
            let ex = victory_metal_back::exec::Executor::new();
            // Check the Mac driver's table now: a mismatch is reported at startup,
            // not when the first swap chain fails.
            if let Err(text) = wine::WineSurfaces::check_at_startup()
                && std::env::var_os("VICTORY_METAL_LOG").is_some_and(|v| v != "0")
            {
                eprintln!("victory-metal bridge: window surfaces unavailable: {text}");
            }
            ex.set_surface_provider(Arc::new(wine::WineSurfaces::new()));
            let e: Arc<dyn Link> = Arc::new(ex);
            *slot = Slot::Installed(e.clone());
            e
        }
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
///
/// # Checking the table instead of trusting the numbers
///
/// The slot numbers are a fact about one build of winemac.drv. [`Table::find`]
/// checks them when the first layer is asked for (and the bridge reports the
/// result once at startup):
///
/// * the symbol `macdrv_functions` must be in a loaded `winemac.so`;
/// * every one of the first ten entries must point into that same image, and
///   when the image keeps its local symbols (CrossOver's does) the symbol
///   found at each entry must be the `my_*` wrapper that slot is meant to
///   hold (`my_get_win_data` at 1, ...), which pins the order and the
///   signatures; if the wrappers carry other names the table is a different
///   version and the error says which slot disagrees and what is there;
/// * `get_win_data` must return a record that starts with the `HWND` asked
///   for and holds a client view, which pins the offset of that view.
///
/// Anything else is a clear error, never a call through a wrong pointer.
pub mod wine {
    use std::ffi::{CStr, c_char, c_void};
    use std::sync::Mutex;

    use super::*;

    /// Stages `acquire` can stop at; the number reached is reported back.
    pub const STAGE_NO_TABLE: u32 = 0;
    pub const STAGE_TABLE: u32 = 1;
    pub const STAGE_WIN_DATA: u32 = 2;
    pub const STAGE_DEVICE: u32 = 3;
    pub const STAGE_VIEW: u32 = 4;
    pub const STAGE_LAYER: u32 = 5;

    /// Slots of `struct macdrv_functions_t` this bridge uses, with the
    /// wrapper function that must sit there (`d3dmetal.c`).
    const SLOTS: [(usize, &str); 8] = [
        (1, "my_get_win_data"),
        (2, "my_release_win_data"),
        (3, "my_macdrv_get_cocoa_window"),
        (4, "my_macdrv_create_metal_device"),
        (5, "my_macdrv_release_metal_device"),
        (6, "my_macdrv_view_create_metal_view"),
        (7, "my_macdrv_view_get_metal_layer"),
        (8, "my_macdrv_view_release_metal_view"),
    ];
    /// Offset of `client_cocoa_view` in `struct d3dmetal_macdrv_win_data`
    /// (hwnd, cocoa_window, cocoa_view, then it).
    const CLIENT_VIEW_OFFSET: usize = 24;

    // dyld's image list; libc marks these deprecated in favour of a crate
    // that wraps the same two functions.
    unsafe extern "C" {
        fn _dyld_image_count() -> u32;
        fn _dyld_get_image_name(index: u32) -> *const c_char;
    }

    type F1 = unsafe extern "C" fn(*mut c_void) -> *mut c_void;
    type F2 = unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void;

    /// The functions of the table, after the checks above.
    #[derive(Clone, Copy)]
    pub struct Table {
        get_win_data: F1,
        release_win_data: unsafe extern "C" fn(*mut c_void),
        create_device: unsafe extern "C" fn() -> *mut c_void,
        release_device: unsafe extern "C" fn(*mut c_void),
        create_view: F2,
        get_layer: F1,
        release_view: unsafe extern "C" fn(*mut c_void),
    }

    // Plain function pointers into a loaded image.
    unsafe impl Send for Table {}
    unsafe impl Sync for Table {}

    /// `macdrv_functions` from the winemac.so Wine has already loaded.
    fn symbol() -> Option<*const usize> {
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

    /// The image and (if the image keeps it) the symbol an address is in.
    fn locate(addr: usize) -> Option<(usize, Option<String>)> {
        let mut info: libc::Dl_info = unsafe { std::mem::zeroed() };
        // SAFETY: `dladdr` fills `info`; the strings it points at live as long as the image.
        unsafe {
            if libc::dladdr(addr as *const c_void, &mut info) == 0 {
                return None;
            }
            let name = (!info.dli_sname.is_null()).then(|| {
                CStr::from_ptr(info.dli_sname)
                    .to_string_lossy()
                    .trim_start_matches('_')
                    .to_string()
            });
            Some((info.dli_fbase as usize, name))
        }
    }

    impl Table {
        /// Finds and checks the table. The error says what is wrong and
        /// where, for the log.
        pub fn find() -> Result<Table, String> {
            let base = symbol().ok_or_else(|| {
                "no `macdrv_functions` in a loaded winemac.so: this Wine is not the CrossOver build with the D3DMetal hooks, or the Mac driver is not in use (Wine is not running on macOS drivers)".to_string()
            })?;
            let (image, _) = locate(base as usize)
                .ok_or_else(|| "cannot tell which image `macdrv_functions` is in".to_string())?;
            let mut at = [0usize; 9];
            for (index, expect) in SLOTS {
                // SAFETY: the table has at least ten pointers (`d3dmetal.c`); the first nine are read.
                let p = unsafe { *base.add(index) };
                if p == 0 {
                    return Err(format!(
                        "macdrv_functions[{index}] is null (expected {expect}): the table is not this bridge's version"
                    ));
                }
                match locate(p) {
                    Some((fbase, _)) if fbase != image => {
                        return Err(format!(
                            "macdrv_functions[{index}] points outside winemac.so (expected {expect}): the table is not this bridge's version"
                        ));
                    }
                    Some((_, Some(name))) if name != expect => {
                        return Err(format!(
                            "macdrv_functions[{index}] is `{name}`, this bridge expects `{expect}`: winemac.drv's D3DMetal table has another layout (another CrossOver version?)"
                        ));
                    }
                    // No symbol name (stripped): the image check above is all there is.
                    _ => {}
                }
                at[index] = p;
            }
            // SAFETY: the wrappers sit where `d3dmetal.c` puts them (checked above)
            // and have the signatures it gives them.
            unsafe {
                Ok(Table {
                    get_win_data: std::mem::transmute::<usize, F1>(at[1]),
                    release_win_data: std::mem::transmute::<usize, unsafe extern "C" fn(*mut c_void)>(
                        at[2],
                    ),
                    create_device: std::mem::transmute::<
                        usize,
                        unsafe extern "C" fn() -> *mut c_void,
                    >(at[4]),
                    release_device: std::mem::transmute::<usize, unsafe extern "C" fn(*mut c_void)>(
                        at[5],
                    ),
                    create_view: std::mem::transmute::<usize, F2>(at[6]),
                    get_layer: std::mem::transmute::<usize, F1>(at[7]),
                    release_view: std::mem::transmute::<usize, unsafe extern "C" fn(*mut c_void)>(
                        at[8],
                    ),
                })
            }
        }
    }

    /// What `Table::acquire` made for one window.
    #[derive(Clone, Copy, Debug)]
    pub struct Acquired {
        pub layer: *mut c_void,
        pub view: *mut c_void,
        pub device: *mut c_void,
    }

    impl Table {
        /// The layer, view and device of the client area of `hwnd`. On an
        /// error: the stage reached and what went wrong.
        ///
        /// # Safety
        /// `hwnd` is a window of this process.
        pub unsafe fn acquire(&self, hwnd: u64) -> Result<Acquired, (u32, String)> {
            // SAFETY: functions checked in `find`; the sequence is D3DMetal's.
            unsafe {
                let data = (self.get_win_data)(hwnd as *mut c_void);
                if data.is_null() {
                    return Err((
                        STAGE_TABLE,
                        format!(
                            "get_win_data({hwnd:#x}) returned null: not a window of the Mac driver"
                        ),
                    ));
                }
                let first = *data.cast::<usize>();
                let client_view = *data
                    .cast::<u8>()
                    .add(CLIENT_VIEW_OFFSET)
                    .cast::<*mut c_void>();
                if first != hwnd as usize || client_view.is_null() {
                    (self.release_win_data)(data);
                    return Err((
                        STAGE_TABLE,
                        format!(
                            "the window record of {hwnd:#x} does not have the layout this bridge knows (first field {first:#x}, client view {client_view:p}): another winemac.drv version"
                        ),
                    ));
                }
                let device = (self.create_device)();
                if device.is_null() {
                    (self.release_win_data)(data);
                    return Err((STAGE_WIN_DATA, "create_metal_device returned null".into()));
                }
                let view = (self.create_view)(client_view, device);
                if view.is_null() {
                    (self.release_device)(device);
                    (self.release_win_data)(data);
                    return Err((STAGE_DEVICE, "view_create_metal_view returned null".into()));
                }
                let layer = (self.get_layer)(view);
                (self.release_win_data)(data);
                if layer.is_null() {
                    (self.release_view)(view);
                    (self.release_device)(device);
                    return Err((STAGE_VIEW, "view_get_metal_layer returned null".into()));
                }
                Ok(Acquired {
                    layer,
                    view,
                    device,
                })
            }
        }

        /// Gives back what `acquire` made.
        ///
        /// # Safety
        /// `a` came from `acquire` of this table and is not released yet.
        pub unsafe fn release(&self, a: Acquired) {
            // SAFETY: as in `acquire`.
            unsafe {
                (self.release_view)(a.view);
                (self.release_device)(a.device);
            }
        }
    }

    /// Fills in the layer, view and device of the window in `args.hwnd`.
    ///
    /// # Safety
    /// `args` points to a `LayerArgs`; `hwnd` is a window of this process.
    pub unsafe extern "C" fn layer_acquire(args: *mut c_void) -> i32 {
        // SAFETY: the PE passes a `LayerArgs`.
        let args = unsafe { &mut *args.cast::<LayerArgs>() };
        args.stage = STAGE_NO_TABLE;
        let table = match Table::find() {
            Ok(t) => t,
            Err(text) => {
                log_once(&text);
                return STATUS_NOT_SUPPORTED;
            }
        };
        args.stage = STAGE_TABLE;
        // SAFETY: `hwnd` is the caller's window.
        match unsafe { table.acquire(args.hwnd) } {
            Ok(a) => {
                args.stage = STAGE_LAYER;
                args.layer = a.layer as u64;
                args.view = a.view as u64;
                args.device = a.device as u64;
                STATUS_SUCCESS
            }
            Err((stage, text)) => {
                args.stage = stage;
                log_once(&text);
                STATUS_UNSUCCESSFUL
            }
        }
    }

    /// Gives back what `layer_acquire` made.
    ///
    /// # Safety
    /// `args` points to the `LayerArgs` that `layer_acquire` filled in.
    pub unsafe extern "C" fn layer_release(args: *mut c_void) -> i32 {
        // SAFETY: the PE passes the `LayerArgs` acquire filled in.
        let args = unsafe { &*args.cast::<LayerArgs>() };
        let Ok(table) = Table::find() else {
            return STATUS_NOT_SUPPORTED;
        };
        // SAFETY: the pointers are what `layer_acquire` gave out.
        unsafe {
            table.release(Acquired {
                layer: args.layer as *mut c_void,
                view: args.view as *mut c_void,
                device: args.device as *mut c_void,
            });
        }
        STATUS_SUCCESS
    }

    /// Writes a reason on stderr (the Wine log of the process), each text once.
    fn log_once(text: &str) {
        static SEEN: Mutex<Vec<String>> = Mutex::new(Vec::new());
        let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
        if !seen.iter().any(|s| s == text) {
            eprintln!("victory-metal bridge: {text}");
            seen.push(text.to_string());
        }
    }

    /// The `SurfaceProvider` of a Wine process: asks winemac.drv for a
    /// Metal view over the client area of the window, keeps it until the
    /// back gives the layer back.
    #[derive(Default)]
    pub struct WineSurfaces {
        held: Mutex<Vec<(u64, Acquired)>>,
    }

    // The pointers are Cocoa objects the table's functions manage.
    unsafe impl Send for WineSurfaces {}
    unsafe impl Sync for WineSurfaces {}

    impl WineSurfaces {
        pub fn new() -> WineSurfaces {
            WineSurfaces::default()
        }

        /// Checks the winemac table now and says what it found, so that a
        /// mismatch shows at startup and not at the first swap chain.
        pub fn check_at_startup() -> Result<(), String> {
            Table::find().map(drop)
        }
    }

    impl victory_metal_proto::SurfaceProvider for WineSurfaces {
        fn layer_for(&self, window: victory_metal_proto::WindowRef) -> *mut c_void {
            let mut held = self.held.lock().unwrap_or_else(|e| e.into_inner());
            if let Some((_, a)) = held.iter().find(|(w, _)| *w == window.0) {
                return a.layer;
            }
            let table = match Table::find() {
                Ok(t) => t,
                Err(text) => {
                    log_once(&text);
                    return std::ptr::null_mut();
                }
            };
            // SAFETY: the window handle comes from the application's swap chain call;
            // `get_win_data` answers null for one it does not know.
            match unsafe { table.acquire(window.0) } {
                Ok(a) => {
                    held.push((window.0, a));
                    a.layer
                }
                Err((_, text)) => {
                    log_once(&text);
                    std::ptr::null_mut()
                }
            }
        }

        fn release_layer(&self, window: victory_metal_proto::WindowRef) {
            let mut held = self.held.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(i) = held.iter().position(|(w, _)| *w == window.0) {
                let (_, a) = held.remove(i);
                if let Ok(table) = Table::find() {
                    // SAFETY: made by `acquire` above and not released before.
                    unsafe { table.release(a) };
                }
            }
        }
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
    fn without_winemac_the_table_check_says_so() {
        let err = wine::Table::find()
            .err()
            .expect("no winemac.so in a native test");
        assert!(
            err.contains("macdrv_functions") && err.contains("winemac.so"),
            "{err}"
        );
        use victory_metal_proto::SurfaceProvider;
        let p = wine::WineSurfaces::new();
        assert!(p.layer_for(victory_metal_proto::WindowRef(5)).is_null());
        // Giving back a layer never handed out is harmless.
        p.release_layer(victory_metal_proto::WindowRef(5));
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
