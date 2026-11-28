//! A small submission rig on the host device (what the tests of `mac/prove` run their compute shaders on): one direct
//! queue, one list, a fence; buffers; a compute pass that reads a table of views and writes words; the words back.

use windows::Win32::Foundation::{HANDLE, WAIT_OBJECT_0};
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
use windows::core::{Interface, s};

use crate::host::{compile, host, transition};

pub(crate) struct Rig {
    pub(crate) device: ID3D12Device,
    queue: ID3D12CommandQueue,
    allocator: ID3D12CommandAllocator,
    pub(crate) list: ID3D12GraphicsCommandList,
    fence: ID3D12Fence,
    event: HANDLE,
    submitted: u64,
}

impl Rig {
    /// `None` without a host GPU.
    pub(crate) fn new() -> Option<Rig> {
        let device = host()?.device.clone();
        // SAFETY: plain object creation on the host device.
        unsafe {
            let queue: ID3D12CommandQueue = device
                .CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC { Type: D3D12_COMMAND_LIST_TYPE_DIRECT, ..Default::default() })
                .ok()?;
            let allocator: ID3D12CommandAllocator = device.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT).ok()?;
            let list: ID3D12GraphicsCommandList = device.CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &allocator, None).ok()?;
            let fence: ID3D12Fence = device.CreateFence(0, D3D12_FENCE_FLAG_NONE).ok()?;
            let event = CreateEventW(None, false, false, None).ok()?;
            Some(Rig { device, queue, allocator, list, fence, event, submitted: 0 })
        }
    }

    /// Records with `record` on the open list, runs it, waits (at most 60 s) and opens the list again.
    pub(crate) fn run(&mut self, record: impl FnOnce(&ID3D12GraphicsCommandList)) {
        record(&self.list);
        self.submitted += 1;
        // SAFETY: our list, queue and fence; the allocator is reset after the GPU finished what was recorded from it.
        unsafe {
            self.list.Close().unwrap();
            let run: ID3D12CommandList = self.list.cast().unwrap();
            self.queue.ExecuteCommandLists(&[Some(run)]);
            self.queue.Signal(&self.fence, self.submitted).unwrap();
            self.fence.SetEventOnCompletion(self.submitted, self.event).unwrap();
            assert!(WaitForSingleObject(self.event, 60_000) == WAIT_OBJECT_0, "a submission did not finish in 60 s");
            let removed = self.device.GetDeviceRemovedReason();
            assert!(removed.is_ok(), "the device is lost: {removed:?}");
            self.allocator.Reset().unwrap();
            self.list.Reset(&self.allocator, None).unwrap();
        }
    }

    pub(crate) fn buffer(&self, kind: D3D12_HEAP_TYPE, flags: D3D12_RESOURCE_FLAGS, state: D3D12_RESOURCE_STATES, bytes: u64) -> ID3D12Resource {
        let heap = D3D12_HEAP_PROPERTIES { Type: kind, ..Default::default() };
        let desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
            Width: bytes,
            Height: 1,
            DepthOrArraySize: 1,
            MipLevels: 1,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
            Flags: flags,
            ..Default::default()
        };
        let mut out: Option<ID3D12Resource> = None;
        // SAFETY: plain buffer creation.
        unsafe { self.device.CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &desc, state, None, &mut out).unwrap() };
        out.unwrap()
    }

    /// Copies the subresources of `texture` (described by `desc`, any state it is in `from`: moved to copy
    /// destination and back to `to`) from `data`, one byte vector per subresource (rows tight, slices tight).
    pub(crate) fn fill_texture(
        &mut self,
        texture: &ID3D12Resource,
        desc: &D3D12_RESOURCE_DESC,
        from: D3D12_RESOURCE_STATES,
        to: D3D12_RESOURCE_STATES,
        data: &[Vec<u8>],
    ) {
        let n = data.len();
        let mut footprints = vec![D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default(); n];
        let mut rows = vec![0u32; n];
        let mut row_bytes = vec![0u64; n];
        let mut total = 0u64;
        // SAFETY: outputs for `n` subresources.
        unsafe {
            self.device.GetCopyableFootprints(desc, 0, n as u32, 0, Some(footprints.as_mut_ptr()), Some(rows.as_mut_ptr()), Some(row_bytes.as_mut_ptr()), Some(&mut total));
        }
        let upload = self.buffer(D3D12_HEAP_TYPE_UPLOAD, D3D12_RESOURCE_FLAG_NONE, D3D12_RESOURCE_STATE_GENERIC_READ, total.max(256));
        // SAFETY: the upload buffer's mapping, `total` bytes.
        unsafe {
            let mut at = std::ptr::null_mut();
            upload.Map(0, None, Some(&mut at)).unwrap();
            let at = at.cast::<u8>();
            std::ptr::write_bytes(at, 0, total as usize);
            for i in 0..n {
                let f = footprints[i].Footprint;
                let row = row_bytes[i] as usize;
                let rows_per_slice = rows[i] as usize;
                let depth = f.Depth.max(1) as usize;
                assert_eq!(
                    data[i].len(),
                    row * rows_per_slice * depth,
                    "subresource {i}: {} bytes of data, {} wanted",
                    data[i].len(),
                    row * rows_per_slice * depth
                );
                for r in 0..rows_per_slice * depth {
                    let (slice, y) = (r / rows_per_slice, r % rows_per_slice);
                    let dst = footprints[i].Offset as usize + slice * f.RowPitch as usize * rows_per_slice + y * f.RowPitch as usize;
                    std::ptr::copy_nonoverlapping(data[i].as_ptr().add(r * row), at.add(dst), row);
                }
            }
            upload.Unmap(0, None);
        }
        self.run(|list| {
            // SAFETY: an open list and live objects.
            unsafe {
                list.ResourceBarrier(&[transition(texture, from, D3D12_RESOURCE_STATE_COPY_DEST)]);
                for i in 0..n {
                    let dst = D3D12_TEXTURE_COPY_LOCATION {
                        pResource: std::mem::transmute_copy(texture),
                        Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
                        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { SubresourceIndex: i as u32 },
                    };
                    let src = D3D12_TEXTURE_COPY_LOCATION {
                        pResource: std::mem::transmute_copy(&upload),
                        Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
                        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { PlacedFootprint: footprints[i] },
                    };
                    list.CopyTextureRegion(&dst, 0, 0, 0, &src, None);
                }
                list.ResourceBarrier(&[transition(texture, D3D12_RESOURCE_STATE_COPY_DEST, to)]);
            }
        });
    }

    /// Moves `texture` between states.
    #[allow(dead_code)]
    pub(crate) fn barrier(&mut self, texture: &ID3D12Resource, before: D3D12_RESOURCE_STATES, after: D3D12_RESOURCE_STATES) {
        self.run(|list| {
            // SAFETY: an open list and a live texture.
            unsafe { list.ResourceBarrier(&[transition(texture, before, after)]) }
        });
    }

    /// Every subresource of `texture` (in state `from`) as tight rows (rows then depth slices), one vector each.
    pub(crate) fn read_texture(&mut self, texture: &ID3D12Resource, desc: &D3D12_RESOURCE_DESC, subresources: u32, from: D3D12_RESOURCE_STATES) -> Vec<Vec<u8>> {
        let n = subresources as usize;
        let mut footprints = vec![D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default(); n];
        let mut rows = vec![0u32; n];
        let mut row_bytes = vec![0u64; n];
        let mut total = 0u64;
        // SAFETY: outputs for `n` subresources.
        unsafe {
            self.device.GetCopyableFootprints(desc, 0, subresources, 0, Some(footprints.as_mut_ptr()), Some(rows.as_mut_ptr()), Some(row_bytes.as_mut_ptr()), Some(&mut total));
        }
        let back = self.buffer(D3D12_HEAP_TYPE_READBACK, D3D12_RESOURCE_FLAG_NONE, D3D12_RESOURCE_STATE_COPY_DEST, total.max(256));
        self.run(|list| {
            // SAFETY: an open list and live objects.
            unsafe {
                list.ResourceBarrier(&[transition(texture, from, D3D12_RESOURCE_STATE_COPY_SOURCE)]);
                for i in 0..n {
                    let src = D3D12_TEXTURE_COPY_LOCATION {
                        pResource: std::mem::transmute_copy(texture),
                        Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
                        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { SubresourceIndex: i as u32 },
                    };
                    let dst = D3D12_TEXTURE_COPY_LOCATION {
                        pResource: std::mem::transmute_copy(&back),
                        Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
                        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { PlacedFootprint: footprints[i] },
                    };
                    list.CopyTextureRegion(&dst, 0, 0, 0, &src, None);
                }
                list.ResourceBarrier(&[transition(texture, D3D12_RESOURCE_STATE_COPY_SOURCE, from)]);
            }
        });
        let mut out = Vec::new();
        // SAFETY: the read-back buffer the finished list copied `total` bytes into.
        unsafe {
            let mut at = std::ptr::null_mut();
            back.Map(0, None, Some(&mut at)).unwrap();
            let at = at.cast::<u8>();
            for i in 0..n {
                let f = footprints[i].Footprint;
                let mut bytes = Vec::new();
                for slice in 0..f.Depth as usize {
                    for y in 0..rows[i] as usize {
                        let from = footprints[i].Offset as usize + slice * f.RowPitch as usize * rows[i] as usize + y * f.RowPitch as usize;
                        bytes.extend_from_slice(std::slice::from_raw_parts(at.add(from), row_bytes[i] as usize));
                    }
                }
                out.push(bytes);
            }
            back.Unmap(0, None);
        }
        out
    }

    /// Runs the compute shader `source` (entry `main`, target cs_5_1; its root signature `RS` is declared in the
    /// source: a table of `views` descriptors from `table`, a root UAV `u0`) in `groups` thread groups of its size, and
    /// reads `words` 32-bit words that it wrote to `u0` back.
    pub(crate) fn compute(
        &mut self,
        source: &str,
        heap: &ID3D12DescriptorHeap,
        table: D3D12_GPU_DESCRIPTOR_HANDLE,
        groups: u32,
        words: usize,
    ) -> Vec<u32> {
        let cs = compile(source, s!("main"), s!("cs_5_1")).unwrap_or_else(|e| panic!("shader: {e:?}\n{source}"));
        // SAFETY: plain object creation from the compiled shader (its root signature is in it).
        let (root, pipeline) = unsafe {
            let root: ID3D12RootSignature = self.device.CreateRootSignature(0, &cs[..]).expect("root signature");
            let pipeline: ID3D12PipelineState = self
                .device
                .CreateComputePipelineState(&D3D12_COMPUTE_PIPELINE_STATE_DESC {
                    pRootSignature: std::mem::transmute_copy(&root),
                    CS: D3D12_SHADER_BYTECODE { pShaderBytecode: cs.as_ptr().cast(), BytecodeLength: cs.len() },
                    ..Default::default()
                })
                .expect("compute pipeline");
            (root, pipeline)
        };
        let bytes = (words * 4).max(16) as u64;
        let out = self.buffer(D3D12_HEAP_TYPE_DEFAULT, D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_UNORDERED_ACCESS, bytes);
        let back = self.buffer(D3D12_HEAP_TYPE_READBACK, D3D12_RESOURCE_FLAG_NONE, D3D12_RESOURCE_STATE_COPY_DEST, bytes);
        self.run(|list| {
            // SAFETY: an open list and live objects.
            unsafe {
                list.SetDescriptorHeaps(&[Some(heap.clone())]);
                list.SetComputeRootSignature(&root);
                list.SetPipelineState(&pipeline);
                list.SetComputeRootDescriptorTable(0, table);
                list.SetComputeRootUnorderedAccessView(1, out.GetGPUVirtualAddress());
                list.Dispatch(groups, 1, 1);
                list.ResourceBarrier(&[transition(&out, D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COPY_SOURCE)]);
                list.CopyBufferRegion(&back, 0, &out, 0, bytes);
            }
        });
        // SAFETY: a read-back buffer the finished list copied `bytes` into.
        unsafe {
            let mut data = std::ptr::null_mut();
            back.Map(0, None, Some(&mut data)).unwrap();
            let read = std::slice::from_raw_parts(data.cast::<u32>(), words).to_vec();
            back.Unmap(0, None);
            read
        }
    }
}
