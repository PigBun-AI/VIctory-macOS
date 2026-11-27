//! Asynchronous pre-Present thumbnails. A descriptor and CPU/GPU storage stay
//! exclusive until both the queue signal and the worker's read have finished.
use crate::host::{Host, transition};
use std::{
    fs::File,
    io::{BufWriter, Write},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    time::{Duration, Instant},
};
use windows::{
    Win32::Graphics::{
        Direct3D::{Fxc::D3DCompile, ID3DBlob},
        Direct3D12::*,
    },
    core::{Error, Result, s},
};
use windows_sys::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
const SIZE: usize = 8448;
const SOURCE: &str = include_str!("frame_thumb.hlsl");
#[cfg(all(test, feature = "diagnostics"))]
#[path = "frame_thumb_tests.rs"]
mod tests;
pub(super) fn enabled() -> bool {
    std::env::var_os("VICTORY_FRAME_THUMB").as_deref() == Some(std::ffi::OsStr::new("1"))
}
struct Slot {
    heap: ID3D12DescriptorHeap,
    out: ID3D12Resource,
    rb: ID3D12Resource,
    reading: Arc<AtomicBool>,
    until: u64,
}
struct Gpu {
    root: ID3D12RootSignature,
    pso: ID3D12PipelineState,
    fence: ID3D12Fence,
    slots: Vec<Slot>,
}
struct Stamp {
    frame: u64,
    present: u64,
    qpc: i64,
}
impl Stamp {
    fn line(&self, off: Option<u64>) -> String {
        format!(
            "frame-thumb frame={} present={} qpc={} off={}",
            self.frame,
            self.present,
            self.qpc,
            off.map_or_else(|| "missing".into(), |v| v.to_string())
        )
    }
}
struct Job {
    stamp: Stamp,
    rb: ID3D12Resource,
    fence: ID3D12Fence,
    reading: Arc<AtomicBool>,
}
pub(super) struct Thumb {
    gpu: Option<Gpu>,
    tx: SyncSender<Job>,
    seq: u64,
}
fn header() -> Vec<u8> {
    let mut freq = 0;
    unsafe { QueryPerformanceFrequency(&mut freq) };
    let mut h = b"VTHUMB01".to_vec();
    for n in [64u16, 36, 256, 24] {
        h.extend(n.to_le_bytes())
    }
    h.extend((SIZE as u32).to_le_bytes());
    h.extend(freq.to_le_bytes());
    h.extend([0; 4]);
    h
}
// A static Host may never drop. Flush already-buffered bytes on normal CRT
// exit if the writer is idle; do not take a fence or wait on a busy worker.
use std::sync::{Mutex, Once, Weak};
type FileBuffer = Mutex<BufWriter<File>>;
static EXIT_FILE: Mutex<Weak<FileBuffer>> = Mutex::new(Weak::new());
extern "C" fn exit_flush() {
    if let Ok(slot) = EXIT_FILE.try_lock() {
        if let Some(file) = slot.upgrade() {
            if let Ok(mut w) = file.try_lock() {
                let _ = w.flush();
            }
        }
    }
}
fn install_flush(w: BufWriter<File>) -> Arc<FileBuffer> {
    static INSTALL: Once = Once::new();
    unsafe extern "C" {
        fn atexit(f: extern "C" fn()) -> i32;
    }
    INSTALL.call_once(|| unsafe {
        let _ = atexit(exit_flush);
    });
    let f = Arc::new(Mutex::new(w));
    *EXIT_FILE.lock().unwrap_or_else(|e| e.into_inner()) = Arc::downgrade(&f);
    f
}
fn writer(rx: Receiver<Job>, path: PathBuf) {
    let mut file = File::create(path)
        .and_then(|f| {
            let mut w = BufWriter::with_capacity(64 * SIZE, f);
            w.write_all(&header())?;
            w.flush()?;
            Ok(install_flush(w))
        })
        .inspect_err(|e| tracing::warn!(%e,"thumbnail file unavailable"))
        .ok();
    let mut off = 32;
    let mut count = 0;
    let mut flushed = Instant::now();
    loop {
        match rx.recv_timeout(Duration::from_secs(1).saturating_sub(flushed.elapsed())) {
            Ok(j) => {
                let completed = loop {
                    let v = unsafe { j.fence.GetCompletedValue() };
                    if v >= j.stamp.present {
                        break v;
                    }
                    std::thread::sleep(Duration::from_millis(1));
                };
                let mut at = None;
                if completed != u64::MAX {
                    if let Some(w) = file.as_mut() {
                        unsafe {
                            let mut p = std::ptr::null_mut();
                            if j.rb.Map(0, None, Some(&mut p)).is_ok() {
                                let res = w
                                    .lock()
                                    .unwrap_or_else(|e| e.into_inner())
                                    .write_all(std::slice::from_raw_parts(p.cast(), SIZE));
                                j.rb.Unmap(0, Some(&D3D12_RANGE { Begin: 0, End: 0 }));
                                if res.is_ok() {
                                    at = Some(off);
                                    off += SIZE as u64;
                                    count += 1
                                } else {
                                    file = None
                                }
                            }
                        }
                    }
                }
                j.reading.store(false, Ordering::Release);
                tracing::warn!("{}", j.stamp.line(at));
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => (),
        }
        if count >= 64 || flushed.elapsed() >= Duration::from_secs(1) {
            if file.as_mut().is_some_and(|w| {
                w.lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .flush()
                    .inspect_err(|e| tracing::warn!(%e,"thumbnail flush failed"))
                    .is_err()
            }) {
                file = None
            }
            flushed = Instant::now();
            count = 0;
        }
    }
}
impl Gpu {
    fn new(host: &Host) -> Result<Self> {
        unsafe {
            let mut blob: Option<ID3DBlob> = None;
            D3DCompile(
                SOURCE.as_ptr().cast(),
                SOURCE.len(),
                s!("frame_thumb"),
                None,
                None,
                s!("main"),
                s!("cs_5_1"),
                0,
                0,
                &mut blob,
                None,
            )?;
            let blob = blob.ok_or_else(Error::empty)?;
            let dev = &host.device;
            let root = dev.CreateRootSignature(
                0,
                std::slice::from_raw_parts(blob.GetBufferPointer().cast(), blob.GetBufferSize()),
            )?;
            let pso = dev.CreateComputePipelineState(&D3D12_COMPUTE_PIPELINE_STATE_DESC {
                pRootSignature: std::mem::transmute_copy(&root),
                CS: D3D12_SHADER_BYTECODE {
                    pShaderBytecode: blob.GetBufferPointer(),
                    BytecodeLength: blob.GetBufferSize(),
                },
                ..Default::default()
            })?;
            let mut slots = Vec::new();
            for _ in 0..4 {
                slots.push(Slot {
                    heap: dev.CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
                        Type: D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV,
                        NumDescriptors: 1,
                        Flags: D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,
                        NodeMask: 0,
                    })?,
                    out: host.uav_buffer(SIZE as u64).ok_or_else(Error::empty)?,
                    rb: host
                        .read_back_buffer(SIZE as u64)
                        .ok_or_else(Error::empty)?,
                    reading: Default::default(),
                    until: 0,
                })
            }
            Ok(Self {
                root,
                pso,
                slots,
                fence: dev.CreateFence(0, D3D12_FENCE_FLAG_NONE)?,
            })
        }
    }
}
impl Thumb {
    pub(super) fn configured(
        host: &Host,
        on: bool,
        path: impl FnOnce() -> PathBuf,
    ) -> Option<Self> {
        on.then(|| Self::new(host, path()))
    }
    fn new(host: &Host, path: PathBuf) -> Self {
        let (tx, rx) = mpsc::sync_channel(4);
        let logs = tracing::dispatcher::get_default(Clone::clone);
        let worker = std::thread::Builder::new()
            .name("frame-thumb".into())
            .spawn(move || tracing::dispatcher::with_default(&logs, || writer(rx, path)));
        if let Err(e) = &worker {
            tracing::warn!(%e, "thumbnail writer unavailable");
        }
        Self {
            gpu: Gpu::new(host)
                .inspect_err(|e| tracing::warn!(%e,"thumbnail compute unavailable"))
                .ok(),
            tx,
            seq: 0,
        }
    }
    pub(super) fn record(
        &mut self,
        host: &Host,
        list: &ID3D12GraphicsCommandList,
        back: &ID3D12Resource,
    ) -> Option<usize> {
        let g = self.gpu.as_mut()?;
        unsafe {
            let done = g.fence.GetCompletedValue();
            if done == u64::MAX {
                return None;
            }
            let i = g
                .slots
                .iter()
                .position(|s| s.until <= done && !s.reading.load(Ordering::Acquire))?;
            let s = &g.slots[i];
            host.device.CreateShaderResourceView(
                back,
                None,
                s.heap.GetCPUDescriptorHandleForHeapStart(),
            );
            let src = D3D12_RESOURCE_STATE_COPY_DEST;
            let read = D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE;
            let uav = D3D12_RESOURCE_STATE_UNORDERED_ACCESS;
            let copy = D3D12_RESOURCE_STATE_COPY_SOURCE;
            list.ResourceBarrier(&[
                transition(back, src, read),
                transition(&s.out, D3D12_RESOURCE_STATE_COMMON, uav),
            ]);
            list.SetDescriptorHeaps(&[Some(s.heap.clone())]);
            list.SetComputeRootSignature(&g.root);
            list.SetPipelineState(&g.pso);
            list.SetComputeRootDescriptorTable(0, s.heap.GetGPUDescriptorHandleForHeapStart());
            list.SetComputeRootUnorderedAccessView(1, s.out.GetGPUVirtualAddress());
            list.Dispatch((SIZE / 4) as u32, 1, 1);
            list.ResourceBarrier(&[transition(back, read, src), transition(&s.out, uav, copy)]);
            list.CopyBufferRegion(&s.rb, 0, &s.out, 0, SIZE as u64);
            Some(i)
        }
    }
    pub(super) fn signal(
        &mut self,
        queue: &ID3D12CommandQueue,
        idx: Option<usize>,
    ) -> Option<usize> {
        self.seq += 1;
        let i = idx?;
        let g = self.gpu.as_mut()?;
        let s = &mut g.slots[i];
        s.until = u64::MAX;
        unsafe { queue.Signal(&g.fence, self.seq) }.ok()?;
        s.until = self.seq;
        Some(i)
    }
    pub(super) fn clock() -> i64 {
        let mut qpc = 0;
        unsafe { QueryPerformanceCounter(&mut qpc) };
        qpc
    }
    pub(super) fn at_present(&mut self, frame: u64, idx: Option<usize>, qpc: i64) {
        let stamp = Stamp {
            frame,
            present: self.seq,
            qpc,
        };
        if let Some((g, i)) = self.gpu.as_ref().zip(idx) {
            let s = &g.slots[i];
            s.reading.store(true, Ordering::Release);
            if let Err(e) = self.tx.try_send(Job {
                stamp,
                rb: s.rb.clone(),
                fence: g.fence.clone(),
                reading: s.reading.clone(),
            }) {
                let j = match e {
                    mpsc::TrySendError::Full(j) | mpsc::TrySendError::Disconnected(j) => j,
                };
                s.reading.store(false, Ordering::Release);
                tracing::warn!("{}", j.stamp.line(None));
            }
        } else {
            tracing::warn!("{}", stamp.line(None))
        }
    }
}
