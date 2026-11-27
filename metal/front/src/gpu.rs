//! The front's side of `victory_metal_proto::Link`: typed sync calls and the
//! `Status` → `HRESULT` mapping.

use std::sync::Arc;

use victory_metal_proto::sync::*;
use victory_metal_proto::{Link, LinkExt, Status};

use crate::hresult::{Hr, Result};

pub fn status_to_hr(s: Status) -> Hr {
    match s {
        Status::OK => Hr::S_OK,
        Status::INVALID_ARG
        | Status::BAD_VERSION
        | Status::NOT_FOUND
        | Status::BAD_STREAM
        | Status::CONFLICT => Hr::E_INVALIDARG,
        Status::UNSUPPORTED | Status::UNKNOWN_OP => Hr::E_NOTIMPL,
        Status::OUT_OF_MEMORY => Hr::E_OUTOFMEMORY,
        Status::DEVICE_LOST => Hr::DXGI_ERROR_DEVICE_REMOVED,
        Status::DEVICE_HUNG => Hr::DXGI_ERROR_DEVICE_HUNG,
        Status::NOT_INITIALIZED => Hr::DXGI_ERROR_INVALID_CALL,
        _ => Hr::E_FAIL,
    }
}

/// A `Link` with the sync-call plumbing the front uses everywhere.
#[derive(Clone)]
pub struct Gpu {
    pub link: Arc<dyn Link>,
}

impl Gpu {
    pub fn new(link: Arc<dyn Link>) -> Gpu {
        Gpu { link }
    }

    /// One sync call; the block holds the outputs on success.
    pub fn call<A: SyncArgs>(&self, args: &mut A) -> Result<()> {
        match self.link.call_args(args) {
            Status::OK => Ok(()),
            s => Err(status_to_hr(s)),
        }
    }

    /// The raw status, for callers that treat some statuses specially
    /// (`FENCE_WAIT` timing out).
    pub fn call_status<A: SyncArgs>(&self, args: &mut A) -> Status {
        self.link.call_args(args)
    }

    pub fn submit(&self, block: &[u8]) -> Result<()> {
        match self.link.submit(block) {
            Status::OK => Ok(()),
            s => Err(status_to_hr(s)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_map_to_the_hresults_callers_check() {
        assert_eq!(status_to_hr(Status::OK), Hr::S_OK);
        assert_eq!(status_to_hr(Status::INVALID_ARG), Hr::E_INVALIDARG);
        assert_eq!(status_to_hr(Status::OUT_OF_MEMORY), Hr::E_OUTOFMEMORY);
        assert_eq!(
            status_to_hr(Status::DEVICE_LOST),
            Hr::DXGI_ERROR_DEVICE_REMOVED
        );
        assert_eq!(status_to_hr(Status::INTERNAL), Hr::E_FAIL);
        assert_eq!(status_to_hr(Status(-99)), Hr::E_FAIL);
    }
}
