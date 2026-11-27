//! The two traits the layers meet at (architecture note 2.4).

use crate::status::Status;
use crate::sync::{SyncArgs, SyncOp};
use core::ffi::c_void;

/// How a front reaches a back. The bridge implements it twice (Wine unixlib
/// across the PE/Mach-O line, and in-process); the back's entry points sit
/// behind it.
pub trait Link: Send + Sync {
    /// Hands a block of commands (`Encoder::finish`) to the back for execution.
    /// The bytes are only borrowed for the call.
    fn submit(&self, block: &[u8]) -> Status;

    /// One synchronous call. `args` points at the op's argument block
    /// (`len` = its size); inputs are read from it and outputs written back
    /// into it. The address is valid in both the caller's and the back's
    /// address space (same process).
    fn call(&self, op: SyncOp, args: *mut u8, len: usize) -> Status;
}

/// Typed `call`: `link.call_args(&mut args)`.
pub trait LinkExt: Link {
    /// Sets the block's head version and calls the op the block belongs to.
    fn call_args<A: SyncArgs>(&self, args: &mut A) -> Status {
        args.head_mut().version = A::VERSION;
        self.call(A::OP, (args as *mut A).cast::<u8>(), A::SIZE)
    }
}

impl<T: Link + ?Sized> LinkExt for T {}

/// An opaque window identifier the front hands to `GET_SURFACE_LAYER`.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct WindowRef(pub u64);

/// Where the back gets the drawing layer of a window from. Installed by the
/// bridge: the Wine implementation asks the window system of the Wine
/// process, Route B makes its own window.
pub trait SurfaceProvider: Send + Sync {
    /// The layer object for `window`, or null. Valid until `release_layer`
    /// or until the window goes away. (Architecture note 2.4 spells it
    /// `metal_layer_for`; proto does not name Metal.)
    fn layer_for(&self, window: WindowRef) -> *mut c_void;

    /// The back is done with the layer it got for `window` (it keeps its
    /// own reference to it until then). Default: nothing to give back.
    fn release_layer(&self, _window: WindowRef) {}
}
