//! The render commands of a stream (`BEGIN_RENDER_PASS` ... `DISPATCH_MESH`):
//! proto's ids and enums to [`crate::draw`]'s plain values.
//!
//! The pass and the draws are the front's to derive; what is checked here is
//! what needs the object tables (a resource that is not a texture, a pipeline
//! made for another root signature, an address outside a live buffer).

use victory_metal_proto::render::{
    BeginRenderPass, CLEAR_DEPTH, CLEAR_STENCIL, ClearAttachment, Draw, DrawIndexed, EndRenderPass,
    IndexFormat, LoadAction, SetIndexBuffer, SetPrimitiveTopology, SetScissors, SetVertexBuffers,
    SetViewports, StoreAction, Topology,
};
use victory_metal_proto::status::Status;
use victory_metal_proto::types::TexViewDim;

use super::stream::{R, Run, write_root_arguments};
use super::{PsoBody, status_of};
use crate::draw::{
    ClearArgs, ColorAttachment, DepthAttachment, DrawArgs, DrawIndexedArgs, Load, PassSpec,
    Primitive, Stores, TargetSpec, ViewportRect,
};
use crate::record::CommandList;
use crate::render::GraphicsPipeline;
use crate::texture::ViewDim;

fn view_dim(d: TexViewDim) -> Result<ViewDim, Status> {
    Ok(match d {
        TexViewDim::TEX1D => ViewDim::D1,
        TexViewDim::TEX1D_ARRAY => ViewDim::D1Array,
        TexViewDim::TEX2D => ViewDim::D2,
        TexViewDim::TEX2D_ARRAY => ViewDim::D2Array,
        TexViewDim::TEX2D_MS => ViewDim::D2Ms,
        TexViewDim::TEX2D_MS_ARRAY => ViewDim::D2MsArray,
        TexViewDim::TEX3D => ViewDim::D3,
        _ => return Err(Status::INVALID_ARG),
    })
}

fn load(l: LoadAction) -> Result<Load, Status> {
    Ok(match l {
        LoadAction::LOAD => Load::Load,
        LoadAction::CLEAR => Load::Clear,
        LoadAction::DONT_CARE => Load::DontCare,
        _ => return Err(Status::INVALID_ARG),
    })
}

fn dont_care(s: StoreAction) -> bool {
    s == StoreAction::DONT_CARE
}

fn primitive(t: Topology) -> Result<Primitive, Status> {
    Ok(match t {
        Topology::POINT_LIST => Primitive::PointList,
        Topology::LINE_LIST => Primitive::LineList,
        Topology::LINE_STRIP => Primitive::LineStrip,
        Topology::TRIANGLE_LIST => Primitive::TriangleList,
        Topology::TRIANGLE_STRIP => Primitive::TriangleStrip,
        _ => return Err(Status::INVALID_ARG),
    })
}

impl Run<'_> {
    pub(super) fn begin_render_pass(&mut self, c: &BeginRenderPass) -> R {
        let target = |st: &super::State,
                      res,
                      format,
                      dim,
                      mip: u8,
                      first_slice,
                      slice_count|
         -> Result<TargetSpec, Status> {
            Ok(TargetSpec {
                texture: st.texture(res)?.clone(),
                format,
                dim: view_dim(dim)?,
                mip: u32::from(mip),
                first_slice,
                slice_count,
            })
        };
        let mut spec = PassSpec::default();
        for a in c.attachments() {
            if a.res.is_null() {
                spec.colors.push(None);
                continue;
            }
            spec.colors.push(Some(ColorAttachment {
                target: target(
                    self.st,
                    a.res,
                    a.format,
                    a.dimension,
                    a.mip,
                    a.first_slice,
                    a.slice_count,
                )?,
                load: load(a.load)?,
                clear: a.clear,
            }));
        }
        let d = &c.ds;
        if !d.res.is_null() {
            spec.depth = Some(DepthAttachment {
                target: target(
                    self.st,
                    d.res,
                    d.format,
                    d.dimension,
                    d.mip,
                    d.first_slice,
                    d.slice_count,
                )?,
                depth_load: load(d.depth_load)?,
                stencil_load: load(d.stencil_load)?,
                clear_depth: d.clear_depth,
                clear_stencil: d.clear_stencil,
            });
        }
        let (list, _) = self.list()?;
        let r = list.begin_pass(spec);
        r.map_err(|e| self.st.refuse("BEGIN_RENDER_PASS", &e))
    }

    pub(super) fn end_render_pass(&mut self, c: &EndRenderPass) -> R {
        let mut stores = Stores::default();
        for (i, s) in c.rt_store.iter().enumerate() {
            stores.color_dont_care[i] = dont_care(*s);
        }
        stores.depth_dont_care = dont_care(c.depth_store);
        stores.stencil_dont_care = dont_care(c.stencil_store);
        let (list, _) = self.list()?;
        list.end_pass(&stores).map_err(|e| status_of(&e))
    }

    /// A state command that only touches the list's shadow.
    pub(super) fn render_state(
        &mut self,
        f: impl FnOnce(&mut CommandList) -> crate::error::Result<()>,
    ) -> R {
        let (list, _) = self.list()?;
        f(list).map_err(|e| status_of(&e))
    }

    pub(super) fn set_viewports(&mut self, c: &SetViewports) -> R {
        let v: Vec<ViewportRect> = c
            .viewports()
            .iter()
            .map(|v| ViewportRect {
                x: v.x,
                y: v.y,
                width: v.width,
                height: v.height,
                min_depth: v.min_depth,
                max_depth: v.max_depth,
            })
            .collect();
        self.render_state(|l| l.set_viewports(&v))
    }

    pub(super) fn set_scissors(&mut self, c: &SetScissors) -> R {
        let r: Vec<[i32; 4]> = c
            .rects()
            .iter()
            .map(|r| [r.left, r.top, r.right, r.bottom])
            .collect();
        self.render_state(|l| l.set_scissors(&r))
    }

    pub(super) fn set_primitive(&mut self, c: &SetPrimitiveTopology) -> R {
        let p = primitive(c.topology)?;
        self.render_state(|l| l.set_primitive(p))
    }

    /// A buffer address range must lie in a live, resident buffer: the GPU
    /// would read memory Metal does not know it must keep.
    fn check_range(&self, va: u64, len: u64) -> R {
        match self.st.device.resolve_va(va, len.max(1)) {
            Some((_, e)) if e.resident => Ok(()),
            _ => Err(Status::INVALID_ARG),
        }
    }

    pub(super) fn set_index_buffer(&mut self, c: &SetIndexBuffer) -> R {
        let wide = match c.format {
            IndexFormat::U16 => false,
            IndexFormat::U32 => true,
            _ => return Err(Status::INVALID_ARG),
        };
        if c.va != 0 {
            let align = if wide { 4 } else { 2 };
            if !c.va.is_multiple_of(align) {
                return Err(Status::INVALID_ARG);
            }
            self.check_range(c.va, u64::from(c.size))?;
        }
        self.render_state(|l| l.set_index_buffer(c.va, c.size, wide))
    }

    pub(super) fn set_vertex_buffers(&mut self, c: &SetVertexBuffers) -> R {
        let mut views = Vec::new();
        for v in c.views() {
            if v.va != 0 {
                self.check_range(v.va, u64::from(v.size))?;
            }
            views.push((v.va, v.size, v.stride));
        }
        self.render_state(|l| l.set_vertex_buffers(c.start_slot, &views))
    }

    /// The bound graphics pipeline and its root arguments, written to the
    /// ring and the argument table.
    fn graphics_ready(&mut self) -> Result<(&mut CommandList, GraphicsPipeline), Status> {
        let (list, bound) = self.list()?;
        let pso = bound.gpso.clone().ok_or(Status::INVALID_ARG)?;
        let sig = bound.graphics.sig.clone().ok_or(Status::INVALID_ARG)?;
        let PsoBody::Graphics(p) = &pso.body else {
            return Err(Status::INVALID_ARG);
        };
        if !pso.root_sig.compatible_with(&sig) {
            return Err(Status::INVALID_ARG);
        }
        let pipeline = p.clone();
        write_root_arguments(list, bound, true)?;
        Ok((list, pipeline))
    }

    pub(super) fn draw(&mut self, c: &Draw) -> R {
        let (list, p) = self.graphics_ready()?;
        let r = list.draw(
            &p,
            DrawArgs {
                vertex_count: c.vertex_count,
                instance_count: c.instance_count,
                start_vertex: c.start_vertex,
                start_instance: c.start_instance,
            },
        );
        r.map_err(|e| self.st.refuse("DRAW", &e))
    }

    pub(super) fn draw_indexed(&mut self, c: &DrawIndexed) -> R {
        let (list, p) = self.graphics_ready()?;
        let r = list.draw_indexed(
            &p,
            DrawIndexedArgs {
                index_count: c.index_count,
                instance_count: c.instance_count,
                start_index: c.start_index,
                base_vertex: c.base_vertex,
                start_instance: c.start_instance,
            },
        );
        r.map_err(|e| self.st.refuse("DRAW_INDEXED", &e))
    }

    pub(super) fn dispatch_mesh(&mut self, c: &victory_metal_proto::render::DispatchMesh) -> R {
        let (list, p) = self.graphics_ready()?;
        let r = list.dispatch_mesh(&p, [c.x, c.y, c.z]);
        r.map_err(|e| self.st.refuse("DISPATCH_MESH", &e))
    }

    pub(super) fn clear_attachment(&mut self, c: &ClearAttachment) -> R {
        let rects: Vec<[i32; 4]> = c
            .rects()
            .iter()
            .map(|r| [r.left, r.top, r.right, r.bottom])
            .collect();
        let (list, _) = self.list()?;
        let r = list.clear_attachment(&ClearArgs {
            target: c.target,
            depth: c.flags & CLEAR_DEPTH != 0,
            stencil: c.flags & CLEAR_STENCIL != 0,
            rects: &rects,
            color: c.color,
            clear_depth: c.clear_depth,
            clear_stencil: c.clear_stencil,
        });
        r.map_err(|e| self.st.refuse("CLEAR_ATTACHMENT", &e))
    }
}
