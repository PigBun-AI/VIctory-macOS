//! Where the front gets its `Link`.
//!
//! The dlls call [`link`] once, on the first `CreateDXGIFactory*` /
//! `D3D12CreateDevice`. The bridge (PE half, `WineUnixlib`) installs its
//! provider with [`set_provider`]; until it does, the front answers with
//! [`crate::mock::MockLink`], so the ABI can be exercised under Wine with no
//! Metal at all. `VICTORY_METAL_LINK=mock` forces the mock once a real
//! provider exists.

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
    let p = *PROVIDER.lock().unwrap();
    match p {
        Some(p) if !forced_mock => p(),
        _ => Ok(Arc::new(crate::mock::MockLink::new())),
    }
}

/// For tests of the provider plumbing.
#[doc(hidden)]
pub fn clear_provider() {
    *PROVIDER.lock().unwrap() = None;
}
