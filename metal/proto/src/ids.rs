//! Handles. Plain `u32` newtypes; 0 is the null handle and never names an object.
//!
//! The back allocates every handle and returns it from a sync call; the front
//! only echoes them in the command stream. `ListId` is the exception: the
//! front picks it, it is only meaningful inside one block.

use crate::status::Error;
use crate::wire::{Reader, Wire};
use alloc::vec::Vec;

macro_rules! handle {
    ($(#[$m:meta])* $name:ident) => {
        $(#[$m])*
        #[repr(transparent)]
        #[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
        pub struct $name(pub u32);

        impl $name {
            pub const NULL: Self = Self(0);
            pub const fn is_null(self) -> bool {
                self.0 == 0
            }
        }

        impl core::fmt::Debug for $name {
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                write!(f, "{}({})", stringify!($name), self.0)
            }
        }

        impl Wire for $name {
            const SIZE: usize = 4;
            const ZERO: Self = Self(0);
            fn put(&self, out: &mut Vec<u8>) {
                self.0.put(out);
            }
            fn get(r: &mut Reader<'_>) -> Result<Self, Error> {
                Ok(Self(u32::get(r)?))
            }
        }
    };
}

handle!(
    /// A buffer: committed, wrapped title memory, or (M2+) a placed resource.
    ResId
);
handle!(
    /// A descriptor heap (CBV/SRV/UAV or sampler).
    HeapId
);
handle!(
    /// A placement heap (memory without descriptors). Reserved for M2.
    MemHeapId
);
handle!(
    /// A root signature.
    RootSigId
);
handle!(
    /// A pipeline state object (compute in M1).
    PsoId
);
handle!(
    /// A fence: a 64-bit counter the queues signal and the CPU can wait on.
    FenceId
);
handle!(
    /// A command queue.
    QueueId
);
handle!(
    /// A command list inside one block. Chosen by the front.
    ListId
);
handle!(
    /// A window surface a layer was fetched for (M3).
    SurfaceId
);
