//! Calls added for the new base (the title's host layer, `victory-gpu`, makes
//! them; the old base did not): PIX markers, `WriteBufferImmediate`, timestamp
//! query heaps, `GetClockCalibration`, `EnqueueMakeResident`,
//! `SetBackgroundProcessingMode`, `DiscardResource`. Each is a thin decode
//! over the core (`crate::timestamps`, `crate::device`); what is approximate
//! is said in `crate::timestamps` and docs/metal/m4-newbase.md.

use std::ffi::c_void;

use windows::Win32::Graphics::Direct3D12::*;
use windows::core::{
    ComObject, IUnknown, IUnknownImpl, Interface, Ref, Result as WResult, implement,
};

use super::device::DeviceObj_Impl;
use super::list::ListObj_Impl;
use super::objects::*;
use super::private::PrivateData;
use super::{OrWerr, return_object, werr, wres};
use crate::hresult::Hr;
use crate::timestamps::{self, QueryHeap};

unsafe extern "system" {
    fn QueryPerformanceCounter(count: *mut i64) -> i32;
}

/// `ID3D12QueryHeap`: a timestamp heap ([`QueryHeap`]).
#[implement(ID3D12QueryHeap)]
pub struct QueryHeapObj {
    pub private: PrivateData,
    pub device_obj: ComObject<super::device::DeviceObj>,
    pub heap: QueryHeap,
}

impl QueryHeapObj_Impl {}

/// The text a PIX call carries (`size` bytes at `data`).
fn marker_text(metadata: u32, data: *const c_void, size: u32) -> String {
    if data.is_null() || size == 0 {
        return String::new();
    }
    // SAFETY: the caller's buffer of `size` bytes.
    let bytes = unsafe { std::slice::from_raw_parts(data as *const u8, size as usize) };
    timestamps::pix_text(metadata, bytes).unwrap_or_default()
}

impl ListObj_Impl {
    pub(super) fn x_SetMarker(&self, metadata: u32, pdata: *const c_void, size: u32) {
        let t = marker_text(metadata, pdata, size);
        self.list.lock().unwrap().marker('=', &t);
    }

    pub(super) fn x_BeginEvent(&self, metadata: u32, pdata: *const c_void, size: u32) {
        let t = marker_text(metadata, pdata, size);
        self.list.lock().unwrap().marker('[', &t);
    }

    pub(super) fn x_EndEvent(&self) {
        self.list.lock().unwrap().marker(']', "");
    }

    pub(super) fn x_WriteBufferImmediate(
        &self,
        count: u32,
        pparams: *const D3D12_WRITEBUFFERIMMEDIATE_PARAMETER,
        pmodes: *const D3D12_WRITEBUFFERIMMEDIATE_MODE,
    ) {
        let mut l = self.list.lock().unwrap();
        if count == 0 {
            return;
        }
        if pparams.is_null() {
            l.fail(Hr::E_INVALIDARG);
            return;
        }
        for i in 0..count as usize {
            // SAFETY: the caller's arrays of `count` entries (`pmodes` may be null).
            let (p, mode) = unsafe {
                (
                    &*pparams.add(i),
                    if pmodes.is_null() {
                        0
                    } else {
                        (*pmodes.add(i)).0 as u32
                    },
                )
            };
            l.write_immediate(p.Dest, p.Value, mode);
        }
    }

    pub(super) fn x_EndQuery(
        &self,
        pqueryheap: Ref<ID3D12QueryHeap>,
        r#type: D3D12_QUERY_TYPE,
        index: u32,
    ) {
        crate::callstats::query_op("end", r#type.0);
        let mut l = self.list.lock().unwrap();
        match pqueryheap
            .as_ref()
            .and_then(|h| h.cast_object_ref::<QueryHeapObj>().ok())
        {
            Some(h) if r#type.0 as u32 == timestamps::QUERY_TYPE_TIMESTAMP => {
                l.end_timestamp(&h.heap, index)
            }
            _ => l.unsupported(),
        }
    }

    pub(super) fn x_ResolveQueryData(
        &self,
        pqueryheap: Ref<ID3D12QueryHeap>,
        r#type: D3D12_QUERY_TYPE,
        startindex: u32,
        numqueries: u32,
        pdestinationbuffer: Ref<ID3D12Resource>,
        aligneddestinationbufferoffset: u64,
    ) {
        crate::callstats::query_op("resolve", r#type.0);
        let mut l = self.list.lock().unwrap();
        let heap = pqueryheap
            .as_ref()
            .and_then(|h| h.cast_object_ref::<QueryHeapObj>().ok());
        let dst = pdestinationbuffer
            .as_ref()
            .and_then(|r| r.cast_object_ref::<ResourceObj>().ok());
        match (heap, dst) {
            (Some(h), Some(d)) if r#type.0 as u32 == timestamps::QUERY_TYPE_TIMESTAMP => l
                .resolve_timestamps(
                    &h.heap,
                    startindex,
                    numqueries,
                    &d.resource,
                    aligneddestinationbufferoffset,
                ),
            _ => l.unsupported(),
        }
    }
}

impl QueueObj_Impl {
    pub(super) fn x_SetMarker(&self, _metadata: u32, _pdata: *const c_void, _size: u32) {}
    pub(super) fn x_BeginEvent(&self, _metadata: u32, _pdata: *const c_void, _size: u32) {}
    pub(super) fn x_EndEvent(&self) {}

    /// The card's tick (nanoseconds of [`timestamps::now`]) and the PC's
    /// performance counter, read together.
    pub(super) fn x_GetClockCalibration(
        &self,
        pgputimestamp: *mut u64,
        pcputimestamp: *mut u64,
    ) -> WResult<()> {
        if pgputimestamp.is_null() || pcputimestamp.is_null() {
            return Err(werr(Hr::E_INVALIDARG));
        }
        let mut qpc = 0i64;
        // SAFETY: a plain query into a local.
        unsafe { QueryPerformanceCounter(&mut qpc) };
        // SAFETY: both out pointers are non-null and the caller's.
        unsafe {
            *pgputimestamp = timestamps::now();
            *pcputimestamp = qpc as u64;
        }
        Ok(())
    }
}

impl DeviceObj_Impl {
    pub(super) fn x_CreateQueryHeap(
        &self,
        pdesc: *const D3D12_QUERY_HEAP_DESC,
        riid: *const windows::core::GUID,
        ppvheap: *mut *mut c_void,
    ) -> WResult<()> {
        if pdesc.is_null() {
            return Err(werr(Hr::E_INVALIDARG));
        }
        // SAFETY: non-null, the caller's description.
        let d = unsafe { &*pdesc };
        let heap = QueryHeap::new(d.Type.0 as u32, d.Count).map_err(werr)?;
        let h = ComObject::new(QueryHeapObj {
            private: PrivateData::default(),
            device_obj: self.to_object(),
            heap,
        });
        return_object(&h.to_interface::<IUnknown>(), riid, ppvheap)
    }

    /// No background processing to switch: accepted, nothing to measure.
    pub(super) fn x_SetBackgroundProcessingMode(
        &self,
        _mode: D3D12_BACKGROUND_PROCESSING_MODE,
        _measurementsaction: D3D12_MEASUREMENTS_ACTION,
        _heventtosignaluponcompletion: windows::Win32::Foundation::HANDLE,
        pbfurthermeasurementsdesired: *mut windows::core::BOOL,
    ) -> WResult<()> {
        if !pbfurthermeasurementsdesired.is_null() {
            // SAFETY: the caller's out pointer.
            unsafe { *pbfurthermeasurementsdesired = false.into() };
        }
        Ok(())
    }

    /// Memory is unified and always resident (`MakeResident` is a no-op too):
    /// the fence is signalled at once.
    pub(super) fn x_EnqueueMakeResident(
        &self,
        _flags: D3D12_RESIDENCY_FLAGS,
        _numobjects: u32,
        _ppobjects: *const Option<ID3D12Pageable>,
        pfencetosignal: Ref<ID3D12Fence>,
        fencevaluetosignal: u64,
    ) -> WResult<()> {
        let f = pfencetosignal.as_ref().or_werr(Hr::E_INVALIDARG)?;
        wres(
            f.cast_object_ref::<FenceObj>()?
                .fence
                .signal(fencevaluetosignal),
        )
    }
}
