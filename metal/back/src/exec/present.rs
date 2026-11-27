//! Swap chain calls (`CREATE_SWAPCHAIN` ... `SET_SWAPCHAIN_LATENCY`): proto
//! handles and statuses around [`crate::present::Swapchain`].
//!
//! Creating and destroying need the window system (the `SurfaceProvider`
//! the bridge installed), which may run code on another thread, so those two
//! run in the [`Executor`] without the state lock held; the rest are state
//! calls.

use std::sync::Arc;

use victory_metal_proto::ids::SurfaceId;
use victory_metal_proto::link::{SurfaceProvider, WindowRef};
use victory_metal_proto::status::Status;
use victory_metal_proto::sync::*;
use victory_metal_proto::types::tex_usage;

use super::{Executor, Res, State, status_of, typed};
use crate::present::{Swapchain, surface_format};
use crate::texture::TextureDesc;

/// A swap chain and what the interpreter remembers about it.
pub(crate) struct SwapEntry {
    chain: Swapchain,
    window: u64,
    format: u32,
    usage: u32,
    count: u32,
}

impl Executor {
    /// Installs where windows' layers come from. The bridge does this once,
    /// before the first `CREATE_SWAPCHAIN`.
    pub fn set_surface_provider(&self, provider: Arc<dyn SurfaceProvider>) {
        *super::lock(&self.surface) = Some(provider);
    }

    fn provider(&self) -> Option<Arc<dyn SurfaceProvider>> {
        super::lock(&self.surface).clone()
    }

    pub(super) fn create_swapchain(&self, a: &mut CreateSwapchainArgs) -> Result<(), Status> {
        if a._pad != 0
            || a.flags & !swapchain_flags::ALL_BITS != 0
            || a.usage & !tex_usage::ALL_BITS != 0
            || a.buffer_count == 0
            || a.buffer_count as usize > MAX_SWAPCHAIN_BUFFERS
            || a.width == 0
            || a.height == 0
        {
            return Err(Status::INVALID_ARG);
        }
        if let Err(e) = surface_format(a.format) {
            self.note_error(format!("CREATE_SWAPCHAIN: {e}"));
            return Err(Status::UNSUPPORTED);
        }
        let Some(provider) = self.provider() else {
            self.note_error("CREATE_SWAPCHAIN: no surface provider installed".into());
            return Err(Status::UNSUPPORTED);
        };
        let layer = provider.layer_for(WindowRef(a.window));
        if layer.is_null() {
            self.note_error(format!(
                "CREATE_SWAPCHAIN: the window system has no drawing layer for window {:#x}",
                a.window
            ));
            return Err(Status::UNSUPPORTED);
        }
        let made = self.with_state(|st| st.make_swapchain(layer, a))?;
        match made {
            Ok(note) => {
                if let Some(n) = note {
                    self.note_error(n);
                }
                Ok(())
            }
            Err((s, note)) => {
                provider.release_layer(WindowRef(a.window));
                if let Some(n) = note {
                    self.note_error(n);
                }
                Err(s)
            }
        }
    }

    pub(super) fn destroy_swapchain(&self, a: &mut DestroySwapchainArgs) -> Result<(), Status> {
        let entry = self.with_state(|st| st.swapchains.remove(a.swapchain.0))?;
        let entry = entry.ok_or(Status::NOT_FOUND)?;
        let window = entry.window;
        drop(entry);
        if let Some(p) = self.provider() {
            p.release_layer(WindowRef(window));
        }
        Ok(())
    }
}

type Made = Result<Option<String>, (Status, Option<String>)>;

impl State {
    fn make_buffers(
        &mut self,
        count: u32,
        w: u32,
        h: u32,
        format: u32,
        usage: u32,
        out: &mut [u32; 16],
    ) -> Result<(), Status> {
        let mut made: Vec<u32> = Vec::new();
        for slot in out.iter_mut().take(count as usize) {
            let desc = TextureDesc::tex2d(
                format,
                w,
                h,
                usage | tex_usage::SRV | tex_usage::RENDER_TARGET,
            );
            let t = match self.device.create_texture(&desc) {
                Ok(t) => t,
                Err(e) => {
                    for id in made {
                        self.resources.remove(id);
                    }
                    return Err(self.refuse("swap chain back buffer", &e));
                }
            };
            match self.resources.insert(Res::Texture(t)) {
                Ok(id) => {
                    made.push(id);
                    *slot = id;
                }
                Err(_) => {
                    for id in made {
                        self.resources.remove(id);
                    }
                    return Err(Status::OUT_OF_MEMORY);
                }
            }
        }
        Ok(())
    }

    fn make_swapchain(
        &mut self,
        layer: *mut std::ffi::c_void,
        a: &mut CreateSwapchainArgs,
    ) -> Made {
        // SAFETY: the provider's contract: a live layer object.
        let chain = unsafe {
            Swapchain::new(
                &self.device,
                layer,
                a.width,
                a.height,
                a.format,
                a.max_latency,
                a.flags & swapchain_flags::PROBE != 0,
            )
        };
        let chain = match chain {
            Ok(c) => c,
            Err(e) => {
                let s = self.refuse("CREATE_SWAPCHAIN", &e);
                return Err((s, self.note.take()));
            }
        };
        let mut buffers = [0u32; 16];
        if let Err(s) = self.make_buffers(
            a.buffer_count,
            a.width,
            a.height,
            a.format,
            a.usage,
            &mut buffers,
        ) {
            return Err((s, self.note.take()));
        }
        let surface_format = chain.info().surface_format;
        let entry = SwapEntry {
            chain,
            window: a.window,
            format: a.format,
            usage: a.usage,
            count: a.buffer_count,
        };
        match self.swapchains.insert(entry) {
            Ok(id) => {
                a.swapchain = SurfaceId(id);
                a.surface_format = surface_format;
                a.buffers = buffers;
                Ok(None)
            }
            Err(_) => {
                for id in buffers.iter().take(a.buffer_count as usize) {
                    self.resources.remove(*id);
                }
                Err((Status::OUT_OF_MEMORY, None))
            }
        }
    }

    pub(super) fn present_call(&mut self, op: SyncOp, args: *mut u8, len: usize) -> Status {
        match op {
            SyncOp::RESIZE_SWAPCHAIN => typed(args, len, |a| self.resize_swapchain(a)),
            SyncOp::PRESENT => typed(args, len, |a| self.present(a)),
            SyncOp::SWAPCHAIN_INFO => typed(args, len, |a| self.swapchain_info(a)),
            SyncOp::SET_SWAPCHAIN_LATENCY => typed::<SetSwapchainLatencyArgs>(args, len, |a| {
                if a.max_latency == 0 || a.max_latency > crate::present::MAX_LATENCY {
                    return Err(Status::INVALID_ARG);
                }
                let e = self
                    .swapchains
                    .get_mut(a.swapchain.0)
                    .ok_or(Status::NOT_FOUND)?;
                e.chain.set_max_latency(a.max_latency);
                Ok(())
            }),
            _ => Status::UNKNOWN_OP,
        }
    }

    fn resize_swapchain(&mut self, a: &mut ResizeSwapchainArgs) -> Result<(), Status> {
        if a.buffer_count as usize > MAX_SWAPCHAIN_BUFFERS
            || a.usage & !tex_usage::ALL_BITS != 0
            || a.width == 0
            || a.height == 0
        {
            return Err(Status::INVALID_ARG);
        }
        let (format, usage, count) = {
            let e = self
                .swapchains
                .get(a.swapchain.0)
                .ok_or(Status::NOT_FOUND)?;
            (
                if a.format == 0 { e.format } else { a.format },
                if a.usage == 0 { e.usage } else { a.usage },
                if a.buffer_count == 0 {
                    e.count
                } else {
                    a.buffer_count
                },
            )
        };
        let mut buffers = [0u32; 16];
        self.make_buffers(count, a.width, a.height, format, usage, &mut buffers)?;
        let res = {
            let e = self
                .swapchains
                .get_mut(a.swapchain.0)
                .ok_or(Status::NOT_FOUND)?;
            e.chain.resize(a.width, a.height, Some(format))
        };
        if let Err(err) = res {
            for id in buffers.iter().take(count as usize) {
                self.resources.remove(*id);
            }
            return Err(self.refuse("RESIZE_SWAPCHAIN", &err));
        }
        let e = self
            .swapchains
            .get_mut(a.swapchain.0)
            .ok_or(Status::NOT_FOUND)?;
        e.format = format;
        e.usage = usage;
        e.count = count;
        a.buffers = buffers;
        Ok(())
    }

    fn present(&mut self, a: &mut PresentArgs) -> Result<(), Status> {
        if a._pad != 0 || a.flags & !present_flags::ALL_BITS != 0 {
            return Err(Status::INVALID_ARG);
        }
        let queue = self
            .queues
            .get(a.queue.0)
            .cloned()
            .ok_or(Status::NOT_FOUND)?;
        let texture = match self.resources.get(a.buffer.0) {
            Some(Res::Texture(t)) => t.clone(),
            Some(_) => return Err(Status::INVALID_ARG),
            None => return Err(Status::NOT_FOUND),
        };
        let e = self
            .swapchains
            .get_mut(a.swapchain.0)
            .ok_or(Status::NOT_FOUND)?;
        let r = e.chain.present(
            &queue,
            &texture,
            a.sync_interval,
            a.flags & present_flags::ALLOW_TEARING != 0,
        );
        r.map_err(|err| {
            self.note = Some(format!("PRESENT: {err}"));
            match err {
                crate::Error::Metal(_) => Status::INTERNAL,
                e => status_of(&e),
            }
        })
    }

    fn swapchain_info(&mut self, a: &mut SwapchainInfoArgs) -> Result<(), Status> {
        let e = self
            .swapchains
            .get(a.swapchain.0)
            .ok_or(Status::NOT_FOUND)?;
        let i = e.chain.info();
        a.width = i.width;
        a.height = i.height;
        a.surface_format = i.surface_format;
        a.probe_valid = u32::from(i.probe_valid);
        a.presents = i.presents;
        a.completed = i.completed;
        a.probe_texel = i.probe_texel;
        a.probe_x = i.probe_x;
        a.probe_y = i.probe_y;
        Ok(())
    }
}
