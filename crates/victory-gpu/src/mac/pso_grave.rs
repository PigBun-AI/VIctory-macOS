//! Diagnostic for a D3DMetal crash (docs/mac/PATCHES.md, G7): which host
//! pipelines are released while the title still runs, and what happens if
//! they are kept alive instead.
//!
//! Releasing a title pipeline drops its host pipeline at once; a host list
//! that recorded `SetPipelineState` with it may still be open, queued or
//! running. `VICTORY_MAC_PSO_GRAVE=1` makes every dropped host pipeline
//! live on in a graveyard (its state object is referenced once more, never
//! released), `=log` only counts and logs the releases, 0 or unset does
//! nothing.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use windows::Win32::Graphics::Direct3D12::ID3D12PipelineState;

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Off,
    Log,
    Keep,
}

fn mode() -> Mode {
    static MODE: OnceLock<Mode> = OnceLock::new();
    *MODE.get_or_init(|| match std::env::var("VICTORY_MAC_PSO_GRAVE").as_deref() {
        Ok("1") => Mode::Keep,
        Ok("log") => Mode::Log,
        _ => Mode::Off,
    })
}

static GRAVE: Mutex<Vec<ID3D12PipelineState>> = Mutex::new(Vec::new());
static RELEASED: AtomicU64 = AtomicU64::new(0);

/// Called as a host pipeline is dropped.
pub(crate) fn dropped(pso: &ID3D12PipelineState, name: &str) {
    let mode = mode();
    if mode == Mode::Off {
        return;
    }
    let n = RELEASED.fetch_add(1, Ordering::Relaxed) + 1;
    tracing::info!(
        n,
        frame = crate::frame::current(),
        pso = format_args!("{:p}", windows::core::Interface::as_raw(pso)),
        shaders = name,
        kept = mode == Mode::Keep,
        "host pipeline released"
    );
    if mode == Mode::Keep {
        GRAVE.lock().unwrap_or_else(|e| e.into_inner()).push(pso.clone());
    }
}
