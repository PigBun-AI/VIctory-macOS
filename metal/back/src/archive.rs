//! Pipeline archives: capture the pipelines the backend compiles into an
//! on-disk archive, and let later compilations find them there.
//!
//! Metal 4 API used (SDK `MacOSX27.2.sdk`, `Metal.framework/Headers`, read
//! 2026-11-27; `objc2-metal` 0.3.2 binds all of it):
//!
//! * `MTL4PipelineDataSetSerializer.h`: `MTL4PipelineDataSetSerializerDescriptor
//!   .configuration` (`CaptureDescriptors` = 1, `CaptureBinaries` = 2, binaries only here) and
//!   `-[MTL4PipelineDataSetSerializer serializeAsArchiveAndFlushToURL:error:]`.
//!   The serializer collects "all relevant data for all pipelines a compiler
//!   instance creates" once it is handed to the compiler through
//!   `MTL4CompilerDescriptor.pipelineDataSetSerializer` (`MTL4Compiler.h:51`).
//!   It has to exist when the compiler is made: [`compiler_descriptor`] is
//!   called by `Device::new`.
//! * `MTLDevice.h:1478`: `-[MTLDevice newArchiveWithURL:error:]` opens the
//!   archive (`MTL4Archive.h`: `MTL4Archive` also has
//!   `newComputePipelineStateWithDescriptor:error:` and
//!   `newRenderPipelineStateWithDescriptor:error:`, the direct lookups).
//! * `MTL4Compiler.h:63`: `MTL4CompilerTaskOptions.lookupArchives`: "the compiler
//!   can reduce the runtime of a compilation task if it finds an entry that
//!   matches a function description within any of the archives". Every
//!   pipeline the backend compiles passes [`task_options`], so one call to
//!   [`set_lookup`] makes the whole backend read the archive; nothing else
//!   changes at the call sites.
//!
//! Capture: call [`arm_capture`] before `Device::new`, compile, then
//! [`flush`]. Replay: [`open`] the archive with the device, [`set_lookup`] it;
//! a pipeline that is not in the archive compiles as before.
//!
//! Process-wide on purpose (a process has one Metal device here): the
//! compiler is made inside `Device::new`, and the compile sites are free
//! functions.

use std::path::Path;
use std::sync::Mutex;

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_foundation::{NSArray, NSString, NSURL};
use objc2_metal::{
    MTL4Archive, MTL4CompilerDescriptor, MTL4CompilerTaskOptions, MTL4PipelineDataSetSerializer,
    MTL4PipelineDataSetSerializerConfiguration, MTL4PipelineDataSetSerializerDescriptor, MTLDevice,
};

use crate::device::Device;
use crate::error::{Error, Result};

/// A Metal object that is shared between threads (Metal objects are).
struct Shared<T>(T);
// SAFETY: MTL4Archive, the serializer and the task options are documented
// thread-safe objects (Metal's compile and lookup entry points take any thread).
unsafe impl<T> Send for Shared<T> {}
unsafe impl<T> Sync for Shared<T> {}

static CAPTURE: Mutex<Capture> = Mutex::new(Capture::Off);
static LOOKUP: Mutex<Option<Shared<Retained<MTL4CompilerTaskOptions>>>> = Mutex::new(None);

enum Capture {
    Off,
    /// The next compiler made gets a serializer.
    Armed,
    /// The compiler has one.
    On(Shared<Retained<ProtocolObject<dyn MTL4PipelineDataSetSerializer>>>),
}

/// The next `Device::new` captures every pipeline it compiles (the
/// binaries). Does nothing to a device that exists already.
pub fn arm_capture() {
    *CAPTURE.lock().unwrap_or_else(|e| e.into_inner()) = Capture::Armed;
}

/// The compiler descriptor `Device::new` makes its compiler from: with the
/// serializer when [`arm_capture`] was called.
pub(crate) fn compiler_descriptor(
    device: &ProtocolObject<dyn MTLDevice>,
) -> Retained<MTL4CompilerDescriptor> {
    let descriptor = MTL4CompilerDescriptor::new();
    let mut capture = CAPTURE.lock().unwrap_or_else(|e| e.into_inner());
    if matches!(*capture, Capture::Armed) {
        let config = MTL4PipelineDataSetSerializerDescriptor::new();
        // Binaries only. The header allows `CaptureDescriptors | CaptureBinaries`,
        // but on macOS 27 / M5 Max `serializeAsArchiveAndFlushToURL:` then
        // returns NO without an NSError (checked 2026-11-27: with 1 or 3 it
        // fails, with 2 it writes the archive).
        config.setConfiguration(MTL4PipelineDataSetSerializerConfiguration::CaptureBinaries);
        let serializer = device.newPipelineDataSetSerializerWithDescriptor(&config);
        descriptor.setPipelineDataSetSerializer(Some(&serializer));
        *capture = Capture::On(Shared(serializer));
    }
    descriptor
}

fn url(path: &Path) -> Retained<NSURL> {
    NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()))
}

/// Writes everything captured so far as an archive at `path` (a file).
pub fn flush(path: &Path) -> Result<()> {
    let capture = CAPTURE.lock().unwrap_or_else(|e| e.into_inner());
    let Capture::On(s) = &*capture else {
        return Err(Error::BadArgument(
            "no capture: call archive::arm_capture before Device::new".into(),
        ));
    };
    s.0.serializeAsArchiveAndFlushToURL_error(&url(path))
        .map_err(|e| {
            Error::Metal(format!(
                "archive {}: {}",
                path.display(),
                e.localizedDescription()
            ))
        })
}

/// An archive opened on a device.
pub struct Archive(Retained<ProtocolObject<dyn MTL4Archive>>);

// SAFETY: see `Shared`.
unsafe impl Send for Archive {}
unsafe impl Sync for Archive {}

/// Opens the archive at `path` on `device`.
pub fn open(device: &Device, path: &Path) -> Result<Archive> {
    device
        .raw()
        .newArchiveWithURL_error(&url(path))
        .map(Archive)
        .map_err(|e| {
            Error::Metal(format!(
                "archive {}: {}",
                path.display(),
                e.localizedDescription()
            ))
        })
}

/// From now on every pipeline compile looks in `archives`, in order, before
/// it compiles. An empty list switches the lookup off.
pub fn set_lookup(archives: &[&Archive]) {
    let mut slot = LOOKUP.lock().unwrap_or_else(|e| e.into_inner());
    if archives.is_empty() {
        *slot = None;
        return;
    }
    let list: Vec<Retained<ProtocolObject<dyn MTL4Archive>>> =
        archives.iter().map(|a| a.0.clone()).collect();
    let options = MTL4CompilerTaskOptions::new();
    options.setLookupArchives(Some(&NSArray::from_retained_slice(&list)));
    *slot = Some(Shared(options));
}

/// The options every pipeline compile passes (`None`: no archive is set).
pub(crate) fn task_options() -> Option<Retained<MTL4CompilerTaskOptions>> {
    LOOKUP
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .map(|o| o.0.clone())
}

thread_local! {
    /// What the compiler calls of this thread took since the last
    /// [`take_compile_us`] (microseconds), for `VICTORY_METAL_DROPSTATS`:
    /// a pipeline found in an archive takes well under a millisecond.
    static COMPILE_US: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Runs a compiler call and adds its time to the thread's tally.
pub(crate) fn timed<T>(f: impl FnOnce() -> T) -> T {
    let t = std::time::Instant::now();
    let r = f();
    COMPILE_US.with(|c| c.set(c.get() + t.elapsed().as_micros() as u64));
    r
}

/// The compiler time of this thread since the last call.
pub(crate) fn take_compile_us() -> u64 {
    COMPILE_US.with(|c| c.replace(0))
}

/// The four archives of `dir` (`cs`, `ms`, `ps`, `vs` `.mtl4a`, what the
/// census wrote), opened and set as the lookup. Returns how many opened.
pub fn open_dir(device: &Device, dir: &Path) -> usize {
    let mut opened = Vec::new();
    for name in ["cs", "ms", "ps", "vs"] {
        let path = dir.join(format!("{name}.mtl4a"));
        let t = std::time::Instant::now();
        match open(device, &path) {
            Ok(a) => {
                eprintln!(
                    "victory-metal back: archive {} opened in {} ms",
                    path.display(),
                    t.elapsed().as_millis()
                );
                opened.push(a);
            }
            Err(e) => eprintln!("victory-metal back: archive not opened: {e}"),
        }
    }
    let refs: Vec<&Archive> = opened.iter().collect();
    set_lookup(&refs);
    // The lookup holds what it needs (the options keep the archives).
    opened.len()
}
