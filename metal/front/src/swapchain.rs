//! `IDXGISwapChain` semantics as plain Rust: what a flip-model swap chain
//! of a D3D12 application is, and how it maps onto the back's swap chain
//! calls. No window types: the shell hands in the window as a number (the
//! `HWND` value), the size when the application leaves it to the window, and
//! decodes the answers.
//!
//! The back makes the back buffers (ordinary textures, `CREATE_SWAPCHAIN`);
//! this side wraps them as [`Resource`]s the application renders into, keeps
//! the index of the current one (it advances with every present, as in the
//! flip model's `FLIP_DISCARD` and `FLIP_SEQUENTIAL`), and makes the
//! `PRESENT` call. Presenting is a sync call: the work already
//! executed on the queue is in the back's hands by then, and the back
//! orders the present after it.
//!
//! Supported: `FLIP_DISCARD` / `FLIP_SEQUENTIAL`, 2 to 16 buffers, one
//! sample, the four formats a game presents (`R8G8B8A8_UNORM`,
//! `B8G8R8A8_UNORM`, `R10G10B10A2_UNORM`, `R16G16B16A16_FLOAT`). Not done:
//! exclusive fullscreen (`SetFullscreenState` answers as windowed),
//! frame statistics, stereo, HDR colour spaces.

use std::sync::{Arc, Mutex};

use victory_metal_proto::sync::*;
use victory_metal_proto::types::tex_usage;
use victory_metal_proto::{QueueId, ResId, SurfaceId};

use crate::device::{Device, Resource};
use crate::gpu::status_to_hr;
use crate::hresult::{Hr, Result};
use crate::resource::{self, ResourceDesc};
use victory_metal_proto::Status;

pub const FORMAT_R10G10B10A2_UNORM: u32 = 24;
pub const FORMAT_R8G8B8A8_UNORM: u32 = 28;
pub const FORMAT_B8G8R8A8_UNORM: u32 = 87;
pub const FORMAT_R16G16B16A16_FLOAT: u32 = 10;

pub const SWAP_EFFECT_FLIP_SEQUENTIAL: u32 = 3;
pub const SWAP_EFFECT_FLIP_DISCARD: u32 = 4;

/// `DXGI_USAGE_UNORDERED_ACCESS`.
pub const USAGE_UNORDERED_ACCESS: u32 = 0x40;
/// `DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT`.
pub const FLAG_FRAME_LATENCY_WAITABLE_OBJECT: u32 = 0x40;
/// `DXGI_SWAP_CHAIN_FLAG_ALLOW_TEARING`.
pub const FLAG_ALLOW_TEARING: u32 = 0x800;

/// `DXGI_PRESENT_TEST`.
pub const PRESENT_TEST: u32 = 0x1;
/// `DXGI_PRESENT_ALLOW_TEARING`.
pub const PRESENT_ALLOW_TEARING: u32 = 0x200;

/// What a swap chain was asked for (`DXGI_SWAP_CHAIN_DESC1` without the
/// parts that do not matter here).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SwapChainDesc {
    pub width: u32,
    pub height: u32,
    pub format: u32,
    pub buffer_count: u32,
    /// `DXGI_USAGE` bits.
    pub buffer_usage: u32,
    pub sample_count: u32,
    pub swap_effect: u32,
    /// `DXGI_SWAP_CHAIN_FLAG` bits.
    pub flags: u32,
}

/// `VICTORY_METAL_PROBE_PRESENT` set (and not 0).
fn probe() -> bool {
    use std::sync::OnceLock;
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_METAL_PROBE_PRESENT").is_some_and(|v| v != "0"))
}

fn check_format(format: u32) -> Result<()> {
    match format {
        FORMAT_R8G8B8A8_UNORM
        | FORMAT_B8G8R8A8_UNORM
        | FORMAT_R10G10B10A2_UNORM
        | FORMAT_R16G16B16A16_FLOAT => Ok(()),
        _ => Err(Hr::DXGI_ERROR_INVALID_CALL),
    }
}

impl SwapChainDesc {
    fn check(&self) -> Result<()> {
        check_format(self.format)?;
        if self.width == 0
            || self.height == 0
            || !(2..=MAX_SWAPCHAIN_BUFFERS as u32).contains(&self.buffer_count)
            || self.sample_count != 1
            || !matches!(
                self.swap_effect,
                SWAP_EFFECT_FLIP_SEQUENTIAL | SWAP_EFFECT_FLIP_DISCARD
            )
        {
            return Err(Hr::DXGI_ERROR_INVALID_CALL);
        }
        Ok(())
    }

    fn usage(&self) -> u32 {
        if self.buffer_usage & USAGE_UNORDERED_ACCESS != 0 {
            tex_usage::UAV
        } else {
            0
        }
    }

    fn waitable(&self) -> bool {
        self.flags & FLAG_FRAME_LATENCY_WAITABLE_OBJECT != 0
    }
}

struct Inner {
    desc: SwapChainDesc,
    ids: Vec<ResId>,
    /// Back buffers nobody asked for yet: they die with the swap chain.
    spare: Vec<Option<Resource>>,
    current: u32,
    presents: u32,
    latency: u32,
}

/// A swap chain on the back.
pub struct SwapChain {
    dev: Arc<Device>,
    id: SurfaceId,
    queue: QueueId,
    window: u64,
    st: Mutex<Inner>,
}

/// Maps the status of a swap chain call: no surface and bad format are not
/// "not implemented" to an application.
fn hr_of(s: Status) -> Hr {
    match s {
        Status::UNSUPPORTED => Hr::DXGI_ERROR_UNSUPPORTED,
        s => status_to_hr(s),
    }
}

fn wrap(
    dev: &Arc<Device>,
    desc: &SwapChainDesc,
    ids: &[u32],
) -> (Vec<ResId>, Vec<Option<Resource>>) {
    let rd = ResourceDesc {
        dimension: resource::DIM_TEXTURE2D,
        alignment: 0,
        width: u64::from(desc.width),
        height: desc.height,
        depth_or_array: 1,
        mips: 1,
        format: desc.format,
        sample_count: 1,
        sample_quality: 0,
        layout: resource::LAYOUT_UNKNOWN,
        flags: resource::FLAG_ALLOW_RENDER_TARGET
            | if desc.usage() != 0 {
                resource::FLAG_ALLOW_UNORDERED_ACCESS
            } else {
                0
            },
    };
    let mut res_ids = Vec::new();
    let mut spare = Vec::new();
    for &id in ids.iter().take(desc.buffer_count as usize) {
        res_ids.push(ResId(id));
        // Back buffers start in the PRESENT (= COMMON) state.
        spare.push(Some(dev.wrap_texture(ResId(id), rd, 0)));
    }
    (res_ids, spare)
}

impl SwapChain {
    /// `CreateSwapChainForHwnd` of a window of the process, presented on `queue`.
    pub fn create(
        dev: &Arc<Device>,
        queue: QueueId,
        window: u64,
        desc: SwapChainDesc,
    ) -> Result<Arc<SwapChain>> {
        desc.check()?;
        let mut a = CreateSwapchainArgs::new();
        a.window = window;
        a.width = desc.width;
        a.height = desc.height;
        a.format = desc.format;
        a.buffer_count = desc.buffer_count;
        a.usage = desc.usage();
        a.flags = if probe() { swapchain_flags::PROBE } else { 0 };
        // The default of a swap chain with a waitable object is one frame.
        a.max_latency = if desc.waitable() { 1 } else { 0 };
        let s = dev.gpu().call_status(&mut a);
        if s != Status::OK {
            return Err(hr_of(s));
        }
        let (ids, spare) = wrap(dev, &desc, &a.buffers);
        Ok(Arc::new(SwapChain {
            dev: dev.clone(),
            id: a.swapchain,
            queue,
            window,
            st: Mutex::new(Inner {
                desc,
                ids,
                spare,
                current: 0,
                presents: 0,
                latency: if desc.waitable() { 1 } else { 3 },
            }),
        }))
    }

    pub fn window(&self) -> u64 {
        self.window
    }

    pub fn desc(&self) -> SwapChainDesc {
        self.st.lock().unwrap().desc
    }

    /// `GetCurrentBackBufferIndex`.
    pub fn current_index(&self) -> u32 {
        self.st.lock().unwrap().current
    }

    /// Number of presents made (`GetLastPresentCount`).
    pub fn present_count(&self) -> u32 {
        self.st.lock().unwrap().presents
    }

    /// The first `GetBuffer` of `index`: the buffer itself, which from then
    /// on belongs to whoever holds it (the shell caches it). `INVALID_CALL`
    /// when it was taken already or the index is out of range.
    pub fn take_buffer(&self, index: u32) -> Result<Resource> {
        let mut st = self.st.lock().unwrap();
        st.spare
            .get_mut(index as usize)
            .and_then(Option::take)
            .ok_or(Hr::DXGI_ERROR_INVALID_CALL)
    }

    /// `Present`: shows the current back buffer and moves to the next one.
    pub fn present(&self, sync_interval: u32, flags: u32) -> Result<()> {
        if sync_interval > 4 {
            return Err(Hr::DXGI_ERROR_INVALID_CALL);
        }
        if flags & PRESENT_TEST != 0 {
            return Ok(());
        }
        let mut st = self.st.lock().unwrap();
        let mut a = PresentArgs::new();
        a.queue = self.queue;
        a.swapchain = self.id;
        a.buffer = st.ids[st.current as usize];
        a.sync_interval = sync_interval;
        if flags & PRESENT_ALLOW_TEARING != 0 {
            a.flags |= present_flags::ALLOW_TEARING;
        }
        let s = self.dev.gpu().call_status(&mut a);
        if s != Status::OK {
            return Err(hr_of(s));
        }
        st.current = (st.current + 1) % st.desc.buffer_count;
        st.presents = st.presents.wrapping_add(1);
        if probe() {
            self.log_probe(st.presents);
        }
        Ok(())
    }

    /// `VICTORY_METAL_PROBE_PRESENT=1`: say on stderr which texel the drawable of
    /// each present held (tests read it; costs the frame overlap).
    fn log_probe(&self, n: u32) {
        let mut a = SwapchainInfoArgs::new();
        a.swapchain = self.id;
        if self.dev.gpu().call_status(&mut a) == Status::OK && a.probe_valid != 0 {
            eprintln!(
                "victory-metal present: frame {n} size {}x{} probe ({},{}) texel {:#018x} surface-format {}",
                a.width, a.height, a.probe_x, a.probe_y, a.probe_texel, a.surface_format
            );
        }
    }

    /// `ResizeBuffers`: new back buffers (the application released its own).
    /// 0 keeps the old width, height, count or format.
    pub fn resize(
        &self,
        width: u32,
        height: u32,
        format: u32,
        buffer_count: u32,
        flags: u32,
    ) -> Result<()> {
        let mut st = self.st.lock().unwrap();
        let mut d = st.desc;
        if width != 0 {
            d.width = width;
        }
        if height != 0 {
            d.height = height;
        }
        if format != 0 {
            d.format = format;
        }
        if buffer_count != 0 {
            d.buffer_count = buffer_count;
        }
        d.flags = flags;
        d.check()?;
        // The old buffers go first (the back destroys them when the GPU is done).
        st.spare.clear();
        let mut a = ResizeSwapchainArgs::new();
        a.swapchain = self.id;
        a.width = d.width;
        a.height = d.height;
        a.format = d.format;
        a.buffer_count = d.buffer_count;
        a.usage = d.usage();
        let s = self.dev.gpu().call_status(&mut a);
        if s != Status::OK {
            return Err(hr_of(s));
        }
        let (ids, spare) = wrap(&self.dev, &d, &a.buffers);
        st.desc = d;
        st.ids = ids;
        st.spare = spare;
        st.current = 0;
        Ok(())
    }

    /// `SetMaximumFrameLatency` (1 to 16; the back keeps up to 8).
    pub fn set_max_latency(&self, n: u32) -> Result<()> {
        if n == 0 || n > 16 {
            return Err(Hr::E_INVALIDARG);
        }
        let mut a = SetSwapchainLatencyArgs::new();
        a.swapchain = self.id;
        a.max_latency = n.min(8);
        let s = self.dev.gpu().call_status(&mut a);
        if s != Status::OK {
            return Err(hr_of(s));
        }
        self.st.lock().unwrap().latency = n;
        Ok(())
    }

    /// `GetMaximumFrameLatency`.
    pub fn max_latency(&self) -> u32 {
        self.st.lock().unwrap().latency
    }
}

impl Drop for SwapChain {
    fn drop(&mut self) {
        // Back buffers still held by the swap chain go with it (their drop
        // destroys them); the surface is given back.
        if let Ok(mut st) = self.st.lock() {
            st.spare.clear();
        }
        let mut a = DestroySwapchainArgs::new();
        a.swapchain = self.id;
        let _ = self.dev.gpu().call_status(&mut a);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::System;
    use crate::mock::MockLink;

    fn dev() -> (Arc<MockLink>, Arc<Device>, crate::device::Queue) {
        let mock = Arc::new(MockLink::new());
        let sys = System::open(mock.clone()).unwrap();
        let dev = Device::new(sys);
        let q = dev.create_queue(0, 0).unwrap();
        (mock, dev, q)
    }

    fn desc() -> SwapChainDesc {
        SwapChainDesc {
            width: 640,
            height: 360,
            format: FORMAT_R8G8B8A8_UNORM,
            buffer_count: 2,
            buffer_usage: 0x20,
            sample_count: 1,
            swap_effect: SWAP_EFFECT_FLIP_DISCARD,
            flags: 0,
        }
    }

    #[test]
    fn buffers_present_in_turn_and_are_destroyed_with_the_chain() {
        let (mock, dev, queue) = dev();
        let q = queue.id;
        let sc = SwapChain::create(&dev, q, 0xbeef, desc()).unwrap();
        assert_eq!(mock.live_textures(), 2);
        assert_eq!(mock.swapchain_window(sc.id), Some(0xbeef));
        let b0 = sc.take_buffer(0).unwrap();
        assert_eq!(sc.take_buffer(0).err(), Some(Hr::DXGI_ERROR_INVALID_CALL));
        assert_eq!(sc.take_buffer(2).err(), Some(Hr::DXGI_ERROR_INVALID_CALL));
        let b1 = sc.take_buffer(1).unwrap();
        assert_eq!(sc.current_index(), 0);
        sc.present(0, 0).unwrap();
        assert_eq!(sc.current_index(), 1);
        sc.present(1, PRESENT_ALLOW_TEARING).unwrap();
        assert_eq!(sc.current_index(), 0);
        let p = mock.presents();
        assert_eq!(p.len(), 2);
        assert_eq!(
            (p[0].buffer, p[0].sync_interval, p[0].flags),
            (b0.res, 0, 0)
        );
        assert_eq!(
            (p[1].buffer, p[1].sync_interval, p[1].flags),
            (b1.res, 1, present_flags::ALLOW_TEARING)
        );
        assert_eq!(sc.present_count(), 2);
        // A test present does nothing.
        sc.present(0, PRESENT_TEST).unwrap();
        assert_eq!(mock.presents().len(), 2);
        drop((b0, b1));
        drop(sc);
        assert_eq!(mock.live_textures(), 0);
        assert_eq!(mock.live_swapchains(), 0);
    }

    #[test]
    fn untaken_buffers_die_with_the_chain() {
        let (mock, dev, queue) = dev();
        let q = queue.id;
        let sc = SwapChain::create(&dev, q, 1, desc()).unwrap();
        drop(sc);
        assert_eq!(mock.live_textures(), 0);
    }

    #[test]
    fn resize_makes_new_buffers_and_keeps_what_is_not_given() {
        let (mock, dev, queue) = dev();
        let q = queue.id;
        let sc = SwapChain::create(&dev, q, 1, desc()).unwrap();
        let old = sc.take_buffer(0).unwrap();
        sc.present(0, 0).unwrap();
        drop(old);
        sc.resize(1280, 0, 0, 3, 0).unwrap();
        let d = sc.desc();
        assert_eq!((d.width, d.height, d.buffer_count), (1280, 360, 3));
        assert_eq!(mock.live_textures(), 3);
        assert_eq!(sc.current_index(), 0);
        let t = mock.texture(sc.take_buffer(2).unwrap().res).unwrap();
        assert_eq!(
            (t.width, t.height, t.format),
            (1280, 360, FORMAT_R8G8B8A8_UNORM)
        );
        assert_eq!(mock.swapchain_size(sc.id), Some((1280, 360)));
        // Bad values are refused before anything happens.
        assert_eq!(
            sc.resize(0, 0, 0, 1, 0).err(),
            Some(Hr::DXGI_ERROR_INVALID_CALL)
        );
        assert_eq!(
            sc.resize(0, 0, 3, 0, 0).err(),
            Some(Hr::DXGI_ERROR_INVALID_CALL)
        );
    }

    #[test]
    fn creation_is_checked_and_back_refusals_are_translated() {
        let (mock, dev, queue) = dev();
        let q = queue.id;
        for bad in [
            SwapChainDesc {
                buffer_count: 1,
                ..desc()
            },
            SwapChainDesc {
                buffer_count: 17,
                ..desc()
            },
            SwapChainDesc {
                format: 91,
                ..desc()
            },
            SwapChainDesc {
                sample_count: 4,
                ..desc()
            },
            SwapChainDesc {
                swap_effect: 0,
                ..desc()
            },
            SwapChainDesc { width: 0, ..desc() },
        ] {
            assert_eq!(
                SwapChain::create(&dev, q, 1, bad).err(),
                Some(Hr::DXGI_ERROR_INVALID_CALL),
                "{bad:?}"
            );
        }
        assert_eq!(mock.live_swapchains(), 0);
        mock.fail_next(SyncOp::CREATE_SWAPCHAIN, Status::UNSUPPORTED);
        assert_eq!(
            SwapChain::create(&dev, q, 1, desc()).err(),
            Some(Hr::DXGI_ERROR_UNSUPPORTED)
        );
        mock.fail_next(SyncOp::PRESENT, Status::DEVICE_LOST);
        let sc = SwapChain::create(&dev, q, 1, desc()).unwrap();
        assert_eq!(sc.present(0, 0).err(), Some(Hr::DXGI_ERROR_DEVICE_REMOVED));
        assert_eq!(sc.current_index(), 0, "a failed present does not advance");
    }

    #[test]
    fn latency_is_kept_and_sent() {
        let (mock, dev, queue) = dev();
        let q = queue.id;
        let waitable = SwapChainDesc {
            flags: FLAG_FRAME_LATENCY_WAITABLE_OBJECT,
            ..desc()
        };
        let sc = SwapChain::create(&dev, q, 1, waitable).unwrap();
        assert_eq!(sc.max_latency(), 1);
        sc.set_max_latency(2).unwrap();
        assert_eq!(sc.max_latency(), 2);
        assert_eq!(mock.swapchain_latency(sc.id), Some(2));
        sc.set_max_latency(16).unwrap();
        assert_eq!(mock.swapchain_latency(sc.id), Some(8));
        assert_eq!(sc.set_max_latency(0).err(), Some(Hr::E_INVALIDARG));
        assert_eq!(sc.set_max_latency(17).err(), Some(Hr::E_INVALIDARG));
    }
}
