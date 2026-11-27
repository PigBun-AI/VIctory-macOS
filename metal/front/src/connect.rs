//! Where the front gets its `Link`.
//!
//! The dlls call [`link`] once, on the first `CreateDXGIFactory*` /
//! `D3D12CreateDevice`:
//!
//! 1. `VICTORY_METAL_LINK=mock` → [`crate::mock::MockLink`]: the ABI can be
//!    exercised under Wine with no Metal at all (the smoke test does).
//! 2. a provider installed with [`set_provider`] (tests, other transports);
//! 3. on Windows targets, the bridge: `WineUnixlib` loaded from
//!    `victory_metal_bridge.dll`, the Wine-marked dll next to the unixlib;
//! 4. elsewhere (native tests) the mock.

use std::sync::{Arc, Mutex};

use victory_metal_proto::Link;

use crate::hresult::Result;

pub type Provider = fn() -> Result<Arc<dyn Link>>;

static PROVIDER: Mutex<Option<Provider>> = Mutex::new(None);

pub fn set_provider(p: Provider) {
    *PROVIDER.lock().unwrap() = Some(p);
}

pub fn link() -> Result<Arc<dyn Link>> {
    let forced_mock = std::env::var("VICTORY_METAL_LINK").is_ok_and(|v| v == "mock");
    if forced_mock {
        return Ok(Arc::new(crate::mock::MockLink::new()));
    }
    let p = *PROVIDER.lock().unwrap();
    match p {
        Some(p) => p(),
        None => default_link(),
    }
}

#[cfg(windows)]
fn default_link() -> Result<Arc<dyn Link>> {
    let l = victory_metal_bridge::pe::WineUnixlib::load("victory_metal_bridge.dll").map_err(|e| {
        // `VICTORY_METAL_LOG=1`: say why (the HRESULT alone does not).
        if std::env::var_os("VICTORY_METAL_LOG").is_some_and(|v| v != "0") {
            eprintln!("victory-metal front: cannot load the bridge: {e:?}");
        }
        crate::hresult::Hr::DXGI_ERROR_UNSUPPORTED
    })?;
    Ok(Arc::new(l))
}

#[cfg(not(windows))]
fn default_link() -> Result<Arc<dyn Link>> {
    Ok(Arc::new(crate::mock::MockLink::new()))
}

/// For tests of the provider plumbing.
#[doc(hidden)]
pub fn clear_provider() {
    *PROVIDER.lock().unwrap() = None;
}
