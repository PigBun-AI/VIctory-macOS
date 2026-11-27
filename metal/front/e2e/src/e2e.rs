//! The checks. Unsafe COM, as in the callers this imitates.
#![allow(clippy::missing_safety_doc)]

use std::ffi::c_void;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
use windows::core::{IUnknown, Interface};

type Res<T> = Result<T, String>;

fn e(what: &str) -> impl Fn(windows::core::Error) -> String + '_ {
    move |err| format!("{what}: {:#010x}", err.code().0 as u32)
}

/// A deterministic byte pattern: `seed` makes two buffers differ.
fn pattern(len: usize, seed: u32) -> Vec<u8> {
    let mut x = seed.wrapping_mul(2654435761).wrapping_add(12345);
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            (x >> 11) as u8
        })
        .collect()
}

struct Env {
    device: ID3D12Device,
    queue: ID3D12CommandQueue,
    alloc: ID3D12CommandAllocator,
    fence: ID3D12Fence,
    event: HANDLE,
    value: u64,
}

impl Env {
    unsafe fn new() -> Res<(Env, String)> {
        unsafe {
            let factory: IDXGIFactory4 = CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0))
                .map_err(e("CreateDXGIFactory2"))?;
            let mut chosen = None;
            let mut name = String::new();
            let mut i = 0;
            while let Ok(a) = factory.EnumAdapters1(i) {
                i += 1;
                if let Ok(d) = a.GetDesc1()
                    && d.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 == 0
                {
                    name = String::from_utf16_lossy(&d.Description)
                        .trim_end_matches('\0')
                        .to_owned();
                    chosen = Some(a);
                    break;
                }
            }
            let adapter = chosen.ok_or("no hardware adapter")?;
            let mut device: Option<ID3D12Device> = None;
            D3D12CreateDevice(&adapter, D3D_FEATURE_LEVEL_12_0, &mut device)
                .map_err(e("D3D12CreateDevice"))?;
            let device = device.ok_or("D3D12CreateDevice returned no device")?;
            let queue: ID3D12CommandQueue = device
                .CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC {
                    Type: D3D12_COMMAND_LIST_TYPE_DIRECT,
                    ..Default::default()
                })
                .map_err(e("CreateCommandQueue"))?;
            let alloc: ID3D12CommandAllocator = device
                .CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT)
                .map_err(e("CreateCommandAllocator"))?;
            let fence: ID3D12Fence = device
                .CreateFence(0, D3D12_FENCE_FLAG_NONE)
                .map_err(e("CreateFence"))?;
            let event = CreateEventW(None, false, false, None).map_err(e("CreateEventW"))?;
            Ok((
                Env {
                    device,
                    queue,
                    alloc,
                    fence,
                    event,
                    value: 0,
                },
                name,
            ))
        }
    }

    unsafe fn list(&self) -> Res<ID3D12GraphicsCommandList> {
        unsafe {
            self.device
                .CreateCommandList(
                    0,
                    D3D12_COMMAND_LIST_TYPE_DIRECT,
                    &self.alloc,
                    None::<&ID3D12PipelineState>,
                )
                .map_err(e("CreateCommandList"))
        }
    }

    unsafe fn buffer(
        &self,
        heap: D3D12_HEAP_TYPE,
        size: u64,
        flags: D3D12_RESOURCE_FLAGS,
        state: D3D12_RESOURCE_STATES,
    ) -> Res<ID3D12Resource> {
        unsafe {
            let mut r: Option<ID3D12Resource> = None;
            self.device
                .CreateCommittedResource(
                    &D3D12_HEAP_PROPERTIES {
                        Type: heap,
                        ..Default::default()
                    },
                    D3D12_HEAP_FLAG_NONE,
                    &D3D12_RESOURCE_DESC {
                        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
                        Width: size,
                        Height: 1,
                        DepthOrArraySize: 1,
                        MipLevels: 1,
                        SampleDesc: DXGI_SAMPLE_DESC {
                            Count: 1,
                            Quality: 0,
                        },
                        Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
                        Flags: flags,
                        ..Default::default()
                    },
                    state,
                    None,
                    &mut r,
                )
                .map_err(e("CreateCommittedResource"))?;
            r.ok_or_else(|| "CreateCommittedResource returned no resource".to_owned())
        }
    }

    unsafe fn upload(&self, size: u64) -> Res<ID3D12Resource> {
        unsafe {
            self.buffer(
                D3D12_HEAP_TYPE_UPLOAD,
                size,
                D3D12_RESOURCE_FLAG_NONE,
                D3D12_RESOURCE_STATE_GENERIC_READ,
            )
        }
    }

    unsafe fn readback(&self, size: u64) -> Res<ID3D12Resource> {
        unsafe {
            self.buffer(
                D3D12_HEAP_TYPE_READBACK,
                size,
                D3D12_RESOURCE_FLAG_NONE,
                D3D12_RESOURCE_STATE_COPY_DEST,
            )
        }
    }

    /// A default-heap buffer, created in COPY_DEST.
    unsafe fn default_buffer(&self, size: u64) -> Res<ID3D12Resource> {
        unsafe {
            self.buffer(
                D3D12_HEAP_TYPE_DEFAULT,
                size,
                D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS,
                D3D12_RESOURCE_STATE_COPY_DEST,
            )
        }
    }

    /// Signal on the queue and wait on the CPU through an event.
    unsafe fn sync(&mut self) -> Res<()> {
        unsafe {
            self.value += 1;
            self.queue
                .Signal(&self.fence, self.value)
                .map_err(e("Signal"))?;
            self.wait(self.value)
        }
    }

    unsafe fn wait(&self, value: u64) -> Res<()> {
        unsafe {
            self.fence
                .SetEventOnCompletion(value, self.event)
                .map_err(e("SetEventOnCompletion"))?;
            let w = WaitForSingleObject(self.event, 20_000);
            if w != WAIT_OBJECT_0 {
                return Err(format!(
                    "wait for fence value {value}: {w:?}, completed {}",
                    self.fence.GetCompletedValue()
                ));
            }
            Ok(())
        }
    }

    unsafe fn run(&mut self, lists: &[&ID3D12GraphicsCommandList]) -> Res<()> {
        unsafe {
            let v: Vec<Option<ID3D12CommandList>> = lists
                .iter()
                .map(|l| l.cast::<ID3D12CommandList>().ok())
                .collect();
            self.queue.ExecuteCommandLists(&v);
            self.sync()
        }
    }
}

unsafe fn write(r: &ID3D12Resource, offset: usize, bytes: &[u8]) -> Res<()> {
    unsafe {
        let mut p: *mut c_void = std::ptr::null_mut();
        r.Map(0, Some(&D3D12_RANGE { Begin: 0, End: 0 }), Some(&mut p))
            .map_err(e("Map (upload)"))?;
        if p.is_null() {
            return Err("Map returned a null pointer".into());
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), p.cast::<u8>().add(offset), bytes.len());
        r.Unmap(
            0,
            Some(&D3D12_RANGE {
                Begin: offset,
                End: offset + bytes.len(),
            }),
        );
        Ok(())
    }
}

unsafe fn read(r: &ID3D12Resource, len: usize) -> Res<Vec<u8>> {
    unsafe {
        let mut p: *mut c_void = std::ptr::null_mut();
        r.Map(0, None, Some(&mut p)).map_err(e("Map (readback)"))?;
        if p.is_null() {
            return Err("Map returned a null pointer".into());
        }
        let v = std::slice::from_raw_parts(p.cast::<u8>(), len).to_vec();
        r.Unmap(0, Some(&D3D12_RANGE { Begin: 0, End: 0 }));
        Ok(v)
    }
}

fn transition(
    r: &ID3D12Resource,
    before: D3D12_RESOURCE_STATES,
    after: D3D12_RESOURCE_STATES,
) -> D3D12_RESOURCE_BARRIER {
    D3D12_RESOURCE_BARRIER {
        Type: D3D12_RESOURCE_BARRIER_TYPE_TRANSITION,
        Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
        Anonymous: D3D12_RESOURCE_BARRIER_0 {
            Transition: std::mem::ManuallyDrop::new(D3D12_RESOURCE_TRANSITION_BARRIER {
                // SAFETY: a borrowed pointer; the barrier never releases it.
                pResource: unsafe { std::mem::transmute_copy(r) },
                Subresource: D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES,
                StateBefore: before,
                StateAfter: after,
            }),
        },
    }
}

const DEST: D3D12_RESOURCE_STATES = D3D12_RESOURCE_STATE_COPY_DEST;
const SRC: D3D12_RESOURCE_STATES = D3D12_RESOURCE_STATE_COPY_SOURCE;

fn check_eq(what: &str, got: &[u8], want: &[u8]) -> Res<()> {
    if got == want {
        return Ok(());
    }
    let at = got.iter().zip(want).position(|(a, b)| a != b);
    Err(format!(
        "{what}: {} bytes differ, first at {:?} (got {:#04x?}, want {:#04x?})",
        got.iter().zip(want).filter(|(a, b)| a != b).count() + got.len().abs_diff(want.len()),
        at,
        at.map(|i| got[i]),
        at.map(|i| want[i])
    ))
}

// ---- the tests ---------------------------------------------------------------

/// The listed sequence: queue, allocator, list, committed upload / default /
/// readback buffers, Map write, CopyBufferRegion, barriers, ExecuteCommandLists,
/// fence Signal / SetEventOnCompletion, Map read.
unsafe fn copy_round_trip(env: &mut Env) -> Res<()> {
    unsafe {
        const N: usize = 64 << 10;
        let up = env.upload(N as u64)?;
        let def = env.default_buffer(N as u64)?;
        let rb = env.readback(N as u64)?;
        let data = pattern(N, 1);
        write(&up, 0, &data)?;
        let list = env.list()?;
        list.CopyBufferRegion(&def, 0, &up, 0, N as u64);
        list.ResourceBarrier(&[transition(&def, DEST, SRC)]);
        list.CopyBufferRegion(&rb, 0, &def, 0, N as u64);
        list.Close().map_err(e("Close"))?;
        env.run(&[&list])?;
        check_eq("readback", &read(&rb, N)?, &data)
    }
}

/// `WriteBufferImmediate` (List2): 32-bit values at addresses inside a default buffer, in
/// order with a copy, in the three modes; the readback shows them.
unsafe fn write_buffer_immediate(env: &mut Env) -> Res<()> {
    unsafe {
        const N: usize = 256;
        let up = env.upload(N as u64)?;
        let def = env.default_buffer(N as u64)?;
        let rb = env.readback(N as u64)?;
        let base = vec![0xEEu8; N];
        write(&up, 0, &base)?;
        let list = env.list()?;
        let list2: ID3D12GraphicsCommandList2 = list
            .cast()
            .map_err(|_| "no ID3D12GraphicsCommandList2".to_owned())?;
        list.CopyBufferRegion(&def, 0, &up, 0, N as u64);
        let va = def.GetGPUVirtualAddress();
        let params = [
            D3D12_WRITEBUFFERIMMEDIATE_PARAMETER {
                Dest: va + 4,
                Value: 0x1111_2222,
            },
            D3D12_WRITEBUFFERIMMEDIATE_PARAMETER {
                Dest: va + 8,
                Value: 0x3333_4444,
            },
            D3D12_WRITEBUFFERIMMEDIATE_PARAMETER {
                Dest: va + 252,
                Value: 0x5555_6666,
            },
        ];
        let modes = [
            D3D12_WRITEBUFFERIMMEDIATE_MODE_DEFAULT,
            D3D12_WRITEBUFFERIMMEDIATE_MODE_MARKER_IN,
            D3D12_WRITEBUFFERIMMEDIATE_MODE_MARKER_OUT,
        ];
        list2.WriteBufferImmediate(params.len() as u32, params.as_ptr(), Some(modes.as_ptr()));
        // The same address twice in one call: the later value wins.
        let again = [
            D3D12_WRITEBUFFERIMMEDIATE_PARAMETER {
                Dest: va + 16,
                Value: 1,
            },
            D3D12_WRITEBUFFERIMMEDIATE_PARAMETER {
                Dest: va + 16,
                Value: 2,
            },
        ];
        list2.WriteBufferImmediate(again.len() as u32, again.as_ptr(), None);
        list.ResourceBarrier(&[transition(&def, DEST, SRC)]);
        list.CopyBufferRegion(&rb, 0, &def, 0, N as u64);
        list.Close().map_err(e("Close"))?;
        env.run(&[&list])?;
        let mut want = base;
        want[4..8].copy_from_slice(&0x1111_2222u32.to_le_bytes());
        want[8..12].copy_from_slice(&0x3333_4444u32.to_le_bytes());
        want[252..256].copy_from_slice(&0x5555_6666u32.to_le_bytes());
        want[16..20].copy_from_slice(&2u32.to_le_bytes());
        check_eq("readback", &read(&rb, N)?, &want)
    }
}

/// Timestamp queries: `EndQuery` and `ResolveQueryData` into a readback buffer, `GetClockCalibration`,
/// and PIX-style markers around them (`BeginEvent` / `SetMarker` / `EndEvent`, metadata 1 = ANSI).
/// Checks only what holds for any clock: non-zero and non-decreasing, a non-zero frequency.
unsafe fn timestamps_and_markers(env: &mut Env) -> Res<()> {
    unsafe {
        let heap: ID3D12QueryHeap = {
            let mut h: Option<ID3D12QueryHeap> = None;
            env.device
                .CreateQueryHeap(
                    &D3D12_QUERY_HEAP_DESC {
                        Type: D3D12_QUERY_HEAP_TYPE_TIMESTAMP,
                        Count: 4,
                        NodeMask: 0,
                    },
                    &mut h,
                )
                .map_err(e("CreateQueryHeap"))?;
            h.ok_or("CreateQueryHeap returned no heap")?
        };
        let rb = env.readback(64)?;
        let list = env.list()?;
        let name = b"e2e pass\0";
        list.BeginEvent(1, Some(name.as_ptr() as *const c_void), name.len() as u32);
        list.EndQuery(&heap, D3D12_QUERY_TYPE_TIMESTAMP, 0);
        list.SetMarker(1, Some(name.as_ptr() as *const c_void), name.len() as u32);
        list.EndQuery(&heap, D3D12_QUERY_TYPE_TIMESTAMP, 1);
        list.EndEvent();
        list.ResolveQueryData(&heap, D3D12_QUERY_TYPE_TIMESTAMP, 0, 2, &rb, 8);
        list.Close().map_err(e("Close"))?;
        let freq = env
            .queue
            .GetTimestampFrequency()
            .map_err(e("GetTimestampFrequency"))?;
        if freq == 0 {
            return Err("GetTimestampFrequency is 0".to_owned());
        }
        let (mut gpu0, mut cpu0) = (0u64, 0u64);
        env.queue
            .GetClockCalibration(&mut gpu0, &mut cpu0)
            .map_err(e("GetClockCalibration"))?;
        env.run(&[&list])?;
        let (mut gpu1, mut cpu1) = (0u64, 0u64);
        env.queue
            .GetClockCalibration(&mut gpu1, &mut cpu1)
            .map_err(e("GetClockCalibration"))?;
        if gpu1 < gpu0 || cpu1 < cpu0 || gpu0 == 0 || cpu0 == 0 {
            return Err(format!(
                "calibration went back or is 0: gpu {gpu0}->{gpu1}, cpu {cpu0}->{cpu1}"
            ));
        }
        let b = read(&rb, 24)?;
        let t = |i: usize| u64::from_le_bytes(b[8 + 8 * i..16 + 8 * i].try_into().unwrap());
        if t(0) == 0 || t(1) < t(0) {
            return Err(format!("timestamps {} then {}", t(0), t(1)));
        }
        if b[..8] != [0; 8] {
            return Err("bytes outside the resolved range changed".to_owned());
        }
        // Queue-side markers and residency calls: accepted.
        env.queue
            .BeginEvent(1, Some(name.as_ptr() as *const c_void), name.len() as u32);
        env.queue.EndEvent();
        Ok(())
    }
}

/// `EnqueueMakeResident` (Device3): the fence reaches the value; `SetBackgroundProcessingMode` (Device6) is accepted.
unsafe fn enqueue_make_resident(env: &mut Env) -> Res<()> {
    unsafe {
        let d3: ID3D12Device3 = env
            .device
            .cast()
            .map_err(|_| "no ID3D12Device3".to_owned())?;
        let buf = env.default_buffer(4096)?;
        let pageable: ID3D12Pageable = buf.cast().map_err(e("cast Pageable"))?;
        d3.EnqueueMakeResident(
            D3D12_RESIDENCY_FLAG_NONE,
            &[Some(pageable)],
            &env.fence,
            env.value + 1,
        )
        .map_err(e("EnqueueMakeResident"))?;
        env.value += 1;
        env.wait(env.value)?;
        if let Ok(d6) = env.device.cast::<ID3D12Device6>() {
            d6.SetBackgroundProcessingMode(
                D3D12_BACKGROUND_PROCESSING_MODE_ALLOWED,
                D3D12_MEASUREMENTS_ACTION_KEEP_ALL,
                None,
                None,
            )
            .map_err(e("SetBackgroundProcessingMode"))?;
        }
        Ok(())
    }
}

/// Sub-range copies at odd offsets and sizes through a default buffer, against a CPU model.
unsafe fn copy_ranges(env: &mut Env) -> Res<()> {
    unsafe {
        const N: usize = 1 << 20;
        let up = env.upload(N as u64)?;
        let def = env.default_buffer(N as u64)?;
        let rb = env.readback(N as u64)?;
        let src = pattern(N, 2);
        write(&up, 0, &src)?;
        let mut model_def = vec![0u8; N];
        let mut model_rb = vec![0u8; N];
        // zero the readback and default buffers first (copy from an upload of zeros)
        let zeros = env.upload(N as u64)?;
        write(&zeros, 0, &model_def)?;
        let list = env.list()?;
        list.CopyBufferRegion(&def, 0, &zeros, 0, N as u64);
        list.CopyBufferRegion(&rb, 0, &zeros, 0, N as u64);
        // (dst offset, src offset, size) upload -> default
        let up_to_def = [
            (0usize, 0usize, 4096usize),
            (10_000, 777, 5000),
            (300_001, 123_457, 100_003),
            (N - 17, N - 17, 17),
            (500_000, 1, 1),
        ];
        for &(d, s, n) in &up_to_def {
            list.CopyBufferRegion(&def, d as u64, &up, s as u64, n as u64);
            model_def[d..d + n].copy_from_slice(&src[s..s + n]);
        }
        list.ResourceBarrier(&[transition(&def, DEST, SRC)]);
        let def_to_rb = [
            (0usize, 0usize, 8192usize),
            (50_000, 299_000, 7_000),
            (900_000, 300_001, 100_003),
            (N - 1, N - 1, 1),
        ];
        for &(d, s, n) in &def_to_rb {
            list.CopyBufferRegion(&rb, d as u64, &def, s as u64, n as u64);
            model_rb[d..d + n].copy_from_slice(&model_def[s..s + n]);
        }
        list.Close().map_err(e("Close"))?;
        env.run(&[&list])?;
        check_eq("readback", &read(&rb, N)?, &model_rb)
    }
}

/// Eight buffers copied in a chain inside one list, with a barrier between links.
unsafe fn copy_chain(env: &mut Env) -> Res<()> {
    unsafe {
        const N: usize = 256 << 10;
        const LINKS: usize = 8;
        let up = env.upload(N as u64)?;
        let rb = env.readback(N as u64)?;
        let bufs: Vec<ID3D12Resource> = (0..LINKS)
            .map(|_| env.default_buffer(N as u64))
            .collect::<Res<_>>()?;
        let data = pattern(N, 3);
        write(&up, 0, &data)?;
        let list = env.list()?;
        list.CopyBufferRegion(&bufs[0], 0, &up, 0, N as u64);
        for i in 1..LINKS {
            list.ResourceBarrier(&[transition(&bufs[i - 1], DEST, SRC)]);
            list.CopyBufferRegion(&bufs[i], 0, &bufs[i - 1], 0, N as u64);
        }
        list.ResourceBarrier(&[transition(&bufs[LINKS - 1], DEST, SRC)]);
        list.CopyBufferRegion(&rb, 0, &bufs[LINKS - 1], 0, N as u64);
        list.Close().map_err(e("Close"))?;
        env.run(&[&list])?;
        check_eq("readback", &read(&rb, N)?, &data)
    }
}

/// Two lists in one ExecuteCommandLists: list 2 reads what list 1 wrote.
unsafe fn two_lists_one_execute(env: &mut Env) -> Res<()> {
    unsafe {
        const N: usize = 128 << 10;
        let up = env.upload(N as u64)?;
        let def = env.default_buffer(N as u64)?;
        let rb = env.readback(N as u64)?;
        let data = pattern(N, 4);
        write(&up, 0, &data)?;
        let alloc2: ID3D12CommandAllocator = env
            .device
            .CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT)
            .map_err(e("CreateCommandAllocator"))?;
        let l1 = env.list()?;
        let l2: ID3D12GraphicsCommandList = env
            .device
            .CreateCommandList(
                0,
                D3D12_COMMAND_LIST_TYPE_DIRECT,
                &alloc2,
                None::<&ID3D12PipelineState>,
            )
            .map_err(e("CreateCommandList"))?;
        l1.CopyBufferRegion(&def, 0, &up, 0, N as u64);
        l1.Close().map_err(e("Close 1"))?;
        l2.ResourceBarrier(&[transition(&def, DEST, SRC)]);
        l2.CopyBufferRegion(&rb, 0, &def, 0, N as u64);
        l2.Close().map_err(e("Close 2"))?;
        env.run(&[&l1, &l2])?;
        check_eq("readback", &read(&rb, N)?, &data)
    }
}

/// Reset of the allocator and the list after the GPU is done, then a second round.
unsafe fn reset_and_reuse(env: &mut Env) -> Res<()> {
    unsafe {
        const N: usize = 4096;
        let up = env.upload(N as u64)?;
        let def = env.default_buffer(N as u64)?;
        let rb = env.readback(N as u64)?;
        let list = env.list()?;
        let mut state_is_src = false;
        for round in 0..3u32 {
            let data = pattern(N, 10 + round);
            write(&up, 0, &data)?;
            if round > 0 {
                env.alloc.Reset().map_err(e("allocator Reset"))?;
                list.Reset(&env.alloc, None::<&ID3D12PipelineState>)
                    .map_err(e("list Reset"))?;
            }
            if state_is_src {
                list.ResourceBarrier(&[transition(&def, SRC, DEST)]);
            }
            list.CopyBufferRegion(&def, 0, &up, 0, N as u64);
            list.ResourceBarrier(&[transition(&def, DEST, SRC)]);
            state_is_src = true;
            list.CopyBufferRegion(&rb, 0, &def, 0, N as u64);
            list.Close().map_err(e("Close"))?;
            env.run(&[&list])?;
            check_eq(&format!("round {round}"), &read(&rb, N)?, &data)?;
        }
        Ok(())
    }
}

/// A queue Wait on a fence the CPU signals later: the work must not run before.
unsafe fn queue_wait_orders_work(env: &mut Env) -> Res<()> {
    unsafe {
        const N: usize = 4096;
        let up = env.upload(N as u64)?;
        let rb = env.readback(N as u64)?;
        let zeros = vec![0u8; N];
        let data = pattern(N, 5);
        write(&up, 0, &data)?;
        // Make the readback zero first.
        let z = env.upload(N as u64)?;
        write(&z, 0, &zeros)?;
        let list = env.list()?;
        list.CopyBufferRegion(&rb, 0, &z, 0, N as u64);
        list.Close().map_err(e("Close"))?;
        env.run(&[&list])?;
        env.alloc.Reset().map_err(e("allocator Reset"))?;
        list.Reset(&env.alloc, None::<&ID3D12PipelineState>)
            .map_err(e("list Reset"))?;

        let gate: ID3D12Fence = env
            .device
            .CreateFence(0, D3D12_FENCE_FLAG_NONE)
            .map_err(e("CreateFence"))?;
        env.queue.Wait(&gate, 7).map_err(e("queue Wait"))?;
        list.CopyBufferRegion(&rb, 0, &up, 0, N as u64);
        list.Close().map_err(e("Close"))?;
        env.queue
            .ExecuteCommandLists(&[Some(list.cast::<ID3D12CommandList>().unwrap())]);
        env.value += 1;
        let v = env.value;
        env.queue.Signal(&env.fence, v).map_err(e("Signal"))?;
        std::thread::sleep(Duration::from_millis(100));
        let done = env.fence.GetCompletedValue();
        if done >= v {
            return Err("the queue ran past a Wait that nothing had signalled".into());
        }
        check_eq("readback before the gate opened", &read(&rb, N)?, &zeros)?;
        gate.Signal(7).map_err(e("fence Signal (CPU)"))?;
        if gate.GetCompletedValue() != 7 {
            return Err(format!(
                "fence value after a CPU Signal(7): {}",
                gate.GetCompletedValue()
            ));
        }
        env.wait(v)?;
        check_eq("readback after the gate opened", &read(&rb, N)?, &data)
    }
}

/// Fence semantics that need no GPU work: CPU Signal, completed values, an event
/// that is already satisfied, a blocking SetEventOnCompletion with no event.
unsafe fn fence_semantics(env: &mut Env) -> Res<()> {
    unsafe {
        let f: ID3D12Fence = env
            .device
            .CreateFence(3, D3D12_FENCE_FLAG_NONE)
            .map_err(e("CreateFence(3)"))?;
        if f.GetCompletedValue() != 3 {
            return Err(format!("initial value {}", f.GetCompletedValue()));
        }
        f.Signal(10).map_err(e("Signal(10)"))?;
        if f.GetCompletedValue() != 10 {
            return Err(format!("after Signal(10): {}", f.GetCompletedValue()));
        }
        // Already satisfied: the event is set at once.
        let ev = CreateEventW(None, true, false, None).map_err(e("CreateEventW"))?;
        f.SetEventOnCompletion(5, ev)
            .map_err(e("SetEventOnCompletion(5)"))?;
        if WaitForSingleObject(ev, 1000) != WAIT_OBJECT_0 {
            return Err("an event for an already completed value was not set".into());
        }
        // Signalled from another thread later.
        let ev2 = CreateEventW(None, true, false, None).map_err(e("CreateEventW"))?;
        f.SetEventOnCompletion(20, ev2)
            .map_err(e("SetEventOnCompletion(20)"))?;
        if WaitForSingleObject(ev2, 50) == WAIT_OBJECT_0 {
            return Err("an event for a future value was set early".into());
        }
        let f2 = f.clone();
        let t = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            f2.Signal(25)
        });
        if WaitForSingleObject(ev2, 5000) != WAIT_OBJECT_0 {
            return Err("an event for a future value was never set".into());
        }
        t.join().unwrap().map_err(e("Signal(25) on a thread"))?;
        // A blocking wait with no event.
        let f3 = f.clone();
        let t = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            f3.Signal(30)
        });
        f.SetEventOnCompletion(30, HANDLE::default())
            .map_err(e("SetEventOnCompletion(30, no event)"))?;
        t.join().unwrap().map_err(e("Signal(30) on a thread"))?;
        if f.GetCompletedValue() < 30 {
            return Err("a blocking SetEventOnCompletion returned early".into());
        }
        Ok(())
    }
}

/// 16 MiB through the default heap, timed.
unsafe fn large_copy(env: &mut Env) -> Res<()> {
    unsafe {
        const N: usize = 16 << 20;
        let up = env.upload(N as u64)?;
        let def = env.default_buffer(N as u64)?;
        let rb = env.readback(N as u64)?;
        let data = pattern(N, 6);
        write(&up, 0, &data)?;
        let list = env.list()?;
        list.CopyBufferRegion(&def, 0, &up, 0, N as u64);
        list.ResourceBarrier(&[transition(&def, DEST, SRC)]);
        list.CopyBufferRegion(&rb, 0, &def, 0, N as u64);
        list.Close().map_err(e("Close"))?;
        let t = Instant::now();
        env.run(&[&list])?;
        let dt = t.elapsed();
        println!(
            "INFO large_copy: 2 x 16 MiB copies in {:.2} ms ({:.1} GB/s)",
            dt.as_secs_f64() * 1e3,
            (2 * N) as f64 / dt.as_secs_f64() / 1e9
        );
        check_eq("readback", &read(&rb, N)?, &data)
    }
}

/// Resource properties an application relies on.
unsafe fn resource_properties(env: &mut Env) -> Res<()> {
    unsafe {
        const N: u64 = 65536;
        let a = env.default_buffer(N)?;
        let b = env.default_buffer(N)?;
        let (va, vb) = (a.GetGPUVirtualAddress(), b.GetGPUVirtualAddress());
        if va == 0 || vb == 0 || va == vb {
            return Err(format!("GPU addresses {va:#x} {vb:#x}"));
        }
        let (lo, hi) = (va.min(vb), va.max(vb));
        if hi - lo < N {
            return Err(format!("two {N}-byte buffers overlap: {va:#x} {vb:#x}"));
        }
        let d = a.GetDesc();
        if d.Dimension != D3D12_RESOURCE_DIMENSION_BUFFER || d.Width != N {
            return Err(format!("GetDesc: {:?} width {}", d.Dimension, d.Width));
        }
        let up = env.upload(N)?;
        let mut p1: *mut c_void = std::ptr::null_mut();
        let mut p2: *mut c_void = std::ptr::null_mut();
        up.Map(0, None, Some(&mut p1)).map_err(e("Map"))?;
        up.Map(0, None, Some(&mut p2)).map_err(e("Map twice"))?;
        if p1 != p2 || p1.is_null() {
            return Err(format!("Map twice: {p1:?} vs {p2:?}"));
        }
        up.Unmap(0, None);
        up.Unmap(0, None);
        // Data written through a mapping is still there after Unmap and a re-Map.
        write(&up, 100, &[1, 2, 3, 4])?;
        let mut p3: *mut c_void = std::ptr::null_mut();
        up.Map(0, None, Some(&mut p3)).map_err(e("Map again"))?;
        let got = std::slice::from_raw_parts(p3.cast::<u8>().add(100), 4).to_vec();
        up.Unmap(0, None);
        check_eq("upload contents after re-Map", &got, &[1, 2, 3, 4])?;
        let hp = a.GetHeapProperties(None, None);
        match hp {
            Ok(()) => {}
            Err(err) => return Err(format!("GetHeapProperties: {:#010x}", err.code().0 as u32)),
        }
        Ok(())
    }
}

/// Descriptor heap, CBV / SRV / UAV on a buffer, CopyDescriptorsSimple, and a
/// root signature through serialize + CreateRootSignature (no pipeline).
unsafe fn descriptors_and_root_signature(env: &mut Env) -> Res<()> {
    unsafe {
        const N: u64 = 65536;
        let buf = env.default_buffer(N)?;
        let heap: ID3D12DescriptorHeap = env
            .device
            .CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
                Type: D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV,
                NumDescriptors: 8,
                Flags: D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,
                NodeMask: 0,
            })
            .map_err(e("CreateDescriptorHeap"))?;
        let inc = env
            .device
            .GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV)
            as usize;
        if inc == 0 {
            return Err("descriptor increment is 0".into());
        }
        let cpu = heap.GetCPUDescriptorHandleForHeapStart();
        let gpu = heap.GetGPUDescriptorHandleForHeapStart();
        if cpu.ptr == 0 || gpu.ptr == 0 {
            return Err("null descriptor heap start".into());
        }
        let at = |i: usize| D3D12_CPU_DESCRIPTOR_HANDLE {
            ptr: cpu.ptr + i * inc,
        };
        env.device.CreateUnorderedAccessView(
            &buf,
            None,
            Some(&D3D12_UNORDERED_ACCESS_VIEW_DESC {
                Format: DXGI_FORMAT_R32_TYPELESS,
                ViewDimension: D3D12_UAV_DIMENSION_BUFFER,
                Anonymous: D3D12_UNORDERED_ACCESS_VIEW_DESC_0 {
                    Buffer: D3D12_BUFFER_UAV {
                        NumElements: (N / 4) as u32,
                        Flags: D3D12_BUFFER_UAV_FLAG_RAW,
                        ..Default::default()
                    },
                },
            }),
            at(0),
        );
        env.device.CreateShaderResourceView(
            &buf,
            Some(&D3D12_SHADER_RESOURCE_VIEW_DESC {
                Format: DXGI_FORMAT_R32_TYPELESS,
                ViewDimension: D3D12_SRV_DIMENSION_BUFFER,
                Shader4ComponentMapping: 0x1688, // D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING
                Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 {
                    Buffer: D3D12_BUFFER_SRV {
                        NumElements: (N / 4) as u32,
                        Flags: D3D12_BUFFER_SRV_FLAG_RAW,
                        ..Default::default()
                    },
                },
            }),
            at(1),
        );
        env.device.CreateConstantBufferView(
            Some(&D3D12_CONSTANT_BUFFER_VIEW_DESC {
                BufferLocation: buf.GetGPUVirtualAddress(),
                SizeInBytes: 256,
            }),
            at(2),
        );
        env.device
            .CopyDescriptorsSimple(3, at(4), at(0), D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV);

        let range = [D3D12_DESCRIPTOR_RANGE1 {
            RangeType: D3D12_DESCRIPTOR_RANGE_TYPE_UAV,
            NumDescriptors: 2,
            BaseShaderRegister: 0,
            RegisterSpace: 0,
            Flags: D3D12_DESCRIPTOR_RANGE_FLAG_NONE,
            OffsetInDescriptorsFromTableStart: 0,
        }];
        let params = [
            D3D12_ROOT_PARAMETER1 {
                ParameterType: D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS,
                Anonymous: D3D12_ROOT_PARAMETER1_0 {
                    Constants: D3D12_ROOT_CONSTANTS {
                        ShaderRegister: 0,
                        RegisterSpace: 0,
                        Num32BitValues: 4,
                    },
                },
                ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
            },
            D3D12_ROOT_PARAMETER1 {
                ParameterType: D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE,
                Anonymous: D3D12_ROOT_PARAMETER1_0 {
                    DescriptorTable: D3D12_ROOT_DESCRIPTOR_TABLE1 {
                        NumDescriptorRanges: 1,
                        pDescriptorRanges: range.as_ptr(),
                    },
                },
                ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
            },
            D3D12_ROOT_PARAMETER1 {
                ParameterType: D3D12_ROOT_PARAMETER_TYPE_SRV,
                Anonymous: D3D12_ROOT_PARAMETER1_0 {
                    Descriptor: D3D12_ROOT_DESCRIPTOR1 {
                        ShaderRegister: 0,
                        RegisterSpace: 0,
                        Flags: D3D12_ROOT_DESCRIPTOR_FLAG_NONE,
                    },
                },
                ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
            },
        ];
        let v = D3D12_VERSIONED_ROOT_SIGNATURE_DESC {
            Version: D3D_ROOT_SIGNATURE_VERSION_1_1,
            Anonymous: D3D12_VERSIONED_ROOT_SIGNATURE_DESC_0 {
                Desc_1_1: D3D12_ROOT_SIGNATURE_DESC1 {
                    NumParameters: params.len() as u32,
                    pParameters: params.as_ptr(),
                    ..Default::default()
                },
            },
        };
        let mut blob: Option<ID3DBlob> = None;
        D3D12SerializeVersionedRootSignature(&v, &mut blob, None)
            .map_err(e("D3D12SerializeVersionedRootSignature"))?;
        let blob = blob.ok_or("no blob")?;
        let bytes =
            std::slice::from_raw_parts(blob.GetBufferPointer().cast::<u8>(), blob.GetBufferSize());
        let _root: ID3D12RootSignature = env
            .device
            .CreateRootSignature(0, bytes)
            .map_err(e("CreateRootSignature"))?;
        // Binding them in a list and executing it must not hurt (no dispatch).
        let list = env.list()?;
        list.SetDescriptorHeaps(&[Some(heap.clone())]);
        list.SetComputeRootSignature(&_root);
        list.SetComputeRoot32BitConstants(0, 4, [1u32, 2, 3, 4].as_ptr().cast(), 0);
        list.SetComputeRootDescriptorTable(1, gpu);
        list.SetComputeRootShaderResourceView(2, buf.GetGPUVirtualAddress());
        list.Close().map_err(e("Close"))?;
        env.run(&[&list])
    }
}

/// An empty list and a list with no commands for the GPU still complete a fence.
unsafe fn empty_execute(env: &mut Env) -> Res<()> {
    unsafe {
        let list = env.list()?;
        list.Close().map_err(e("Close"))?;
        env.run(&[&list])?;
        // Signal with no lists at all.
        env.sync()?;
        // Device is healthy.
        env.device
            .GetDeviceRemovedReason()
            .map_err(e("GetDeviceRemovedReason"))
    }
}

// ---- timing ---------------------------------------------------------------------

fn stats(name: &str, mut us: Vec<f64>) {
    us.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = us.len();
    println!(
        "PERF {name} p50_us={:.1} p99_us={:.1} min_us={:.1} n={n}",
        us[n / 2],
        us[(n * 99 / 100).min(n - 1)],
        us[0]
    );
}

/// ExecuteCommandLists round trips. The closed list is submitted again and again
/// (legal once the previous run is done).
unsafe fn timing(env: &mut Env) -> Res<()> {
    unsafe {
        const WARM: usize = 200;
        const N: usize = 2000;
        let up = env.upload(4096)?;
        let def = env.default_buffer(4096)?;
        let empty = env.list()?;
        empty.Close().map_err(e("Close"))?;
        // One allocator per list, so closing one does not disturb another.
        let alloc_c: ID3D12CommandAllocator = env
            .device
            .CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT)
            .map_err(e("CreateCommandAllocator"))?;
        let copy: ID3D12GraphicsCommandList = env
            .device
            .CreateCommandList(
                0,
                D3D12_COMMAND_LIST_TYPE_DIRECT,
                &alloc_c,
                None::<&ID3D12PipelineState>,
            )
            .map_err(e("CreateCommandList"))?;
        copy.CopyBufferRegion(&def, 0, &up, 0, 4096);
        copy.Close().map_err(e("Close"))?;

        // Round trip: ExecuteCommandLists + Signal, then spin on GetCompletedValue.
        let spin = |env: &mut Env, lists: &[Option<ID3D12CommandList>]| -> Res<f64> {
            env.value += 1;
            let v = env.value;
            let t = Instant::now();
            env.queue.ExecuteCommandLists(lists);
            env.queue.Signal(&env.fence, v).map_err(e("Signal"))?;
            let deadline = Instant::now() + Duration::from_secs(20);
            while env.fence.GetCompletedValue() < v {
                if Instant::now() > deadline {
                    return Err("timing: a fence never completed".into());
                }
                std::hint::spin_loop();
            }
            Ok(t.elapsed().as_secs_f64() * 1e6)
        };
        let empty_l = [empty.cast::<ID3D12CommandList>().ok()];
        let copy_l = [copy.cast::<ID3D12CommandList>().ok()];
        let many: Vec<Option<ID3D12CommandList>> = (0..8).map(|_| copy_l[0].clone()).collect();
        let none: [Option<ID3D12CommandList>; 0] = [];

        for (name, lists) in [
            ("ecl_empty_list_spin", &empty_l[..]),
            ("ecl_copy4k_spin", &copy_l[..]),
            ("ecl_no_lists_spin", &none[..]),
        ] {
            let mut us = Vec::with_capacity(N);
            for i in 0..WARM + N {
                let t = spin(env, lists)?;
                if i >= WARM {
                    us.push(t);
                }
            }
            stats(name, us);
        }
        // Eight lists in one call: is the cost per call or per list?
        let mut us = Vec::with_capacity(N);
        for i in 0..WARM + N {
            let t = spin(env, &many)?;
            if i >= WARM {
                us.push(t);
            }
        }
        stats("ecl_copy4k_x8_spin", us);

        // Round trip through an event: what the callers (victory-gpu) do.
        let mut us = Vec::with_capacity(N);
        for i in 0..WARM + N {
            env.value += 1;
            let v = env.value;
            let t = Instant::now();
            env.queue.ExecuteCommandLists(&empty_l);
            env.queue.Signal(&env.fence, v).map_err(e("Signal"))?;
            env.wait(v)?;
            if i >= WARM {
                us.push(t.elapsed().as_secs_f64() * 1e6);
            }
        }
        stats("ecl_empty_list_event", us);

        // The call alone: 100 submissions with no wait, then one wait. A pool of
        // distinct lists: submitting one list again while its previous run is
        // still in flight is legal D3D12, but D3DMetal 4.0b3 under Wine crashes
        // on it (null write in D3DMetal, about one process in three).
        let pool: Vec<Option<ID3D12CommandList>> = (0..100)
            .map(|_| {
                let l: ID3D12GraphicsCommandList = env
                    .device
                    .CreateCommandList(
                        0,
                        D3D12_COMMAND_LIST_TYPE_DIRECT,
                        &alloc_c,
                        None::<&ID3D12PipelineState>,
                    )
                    .map_err(e("CreateCommandList"))?;
                l.CopyBufferRegion(&def, 0, &up, 0, 4096);
                l.Close().map_err(e("Close"))?;
                Ok(l.cast::<ID3D12CommandList>().ok())
            })
            .collect::<Res<_>>()?;
        let mut us = Vec::new();
        for i in 0..(WARM / 100 + N / 100) {
            let t = Instant::now();
            for l in &pool {
                env.queue.ExecuteCommandLists(std::slice::from_ref(l));
            }
            let dt = t.elapsed().as_secs_f64() * 1e6 / 100.0;
            env.sync()?;
            if i >= WARM / 100 {
                us.push(dt);
            }
        }
        stats("ecl_copy4k_call_only", us);
        Ok(())
    }
}

// ---- driver -----------------------------------------------------------------------

struct Tally {
    pass: u32,
    fail: u32,
    skip: u32,
}

impl Tally {
    fn test(&mut self, name: &str, r: Res<()>) {
        match r {
            Ok(()) => {
                self.pass += 1;
                println!("E2E PASS {name}");
            }
            Err(why) => {
                self.fail += 1;
                println!("E2E FAIL {name}: {why}");
            }
        }
    }

    #[allow(dead_code)]
    fn skip(&mut self, name: &str, why: &str) {
        self.skip += 1;
        println!("E2E SKIP {name}: {why}");
    }
}

pub fn run() -> i32 {
    println!("INFO start: front-e2e");
    let want_perf = !std::env::args().any(|a| a == "--no-perf");
    let mut t = Tally {
        pass: 0,
        fail: 0,
        skip: 0,
    };
    // SAFETY: plain COM calls with valid pointers, as the callers do.
    unsafe {
        let (mut env, adapter) = match Env::new() {
            Ok(v) => v,
            Err(why) => {
                println!("E2E FAIL device_and_queue: {why}");
                println!("E2E SUMMARY pass=0 fail=1 skip=0");
                return 1;
            }
        };
        println!("INFO adapter: {adapter}");
        t.test("device_and_queue", Ok(()));
        t.test("copy_round_trip", copy_round_trip(&mut env));
        t.test("copy_ranges", copy_ranges(&mut env));
        t.test("copy_chain", copy_chain(&mut env));
        t.test("two_lists_one_execute", two_lists_one_execute(&mut env));
        t.test("reset_and_reuse", reset_and_reuse(&mut env));
        t.test("fence_semantics", fence_semantics(&mut env));
        t.test("queue_wait_orders_work", queue_wait_orders_work(&mut env));
        t.test("resource_properties", resource_properties(&mut env));
        t.test(
            "descriptors_and_root_signature",
            descriptors_and_root_signature(&mut env),
        );
        t.test("empty_execute", empty_execute(&mut env));
        t.test("large_copy", large_copy(&mut env));
        t.test("write_buffer_immediate", write_buffer_immediate(&mut env));
        t.test("timestamps_and_markers", timestamps_and_markers(&mut env));
        t.test("enqueue_make_resident", enqueue_make_resident(&mut env));
        if want_perf {
            t.test("timing", timing(&mut env));
        }
        t.test(
            "no_device_removal",
            env.device
                .GetDeviceRemovedReason()
                .map_err(e("GetDeviceRemovedReason")),
        );
        let _ = std::mem::size_of::<IUnknown>();
    }
    println!(
        "E2E SUMMARY pass={} fail={} skip={}",
        t.pass, t.fail, t.skip
    );
    i32::from(t.fail > 0)
}
