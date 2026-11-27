//! The checks. Everything is unsafe COM, as in the callers this imitates.
#![allow(clippy::missing_safety_doc)]

use std::ffi::c_void;
use std::fmt::Write as _;

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
use windows::core::{IUnknown, Interface, PCSTR};

fn gold(tag: &str, text: impl AsRef<str>) {
    println!("GOLD {tag}: {}", text.as_ref());
}

fn info(tag: &str, text: impl AsRef<str>) {
    println!("INFO {tag}: {}", text.as_ref());
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

fn hr<T>(r: &windows::core::Result<T>) -> String {
    match r {
        Ok(_) => "S_OK".into(),
        Err(e) => format!("{:#010x}", e.code().0 as u32),
    }
}

pub fn run() {
    println!("INFO start: front-smoke");
    // SAFETY: plain COM calls with valid pointers, as the callers do.
    unsafe {
        let factory = factory_checks();
        let Some(factory) = factory else {
            println!("GOLD fatal: no factory");
            std::process::exit(2);
        };
        let Some(device) = device_checks(&factory) else {
            println!("GOLD fatal: no device");
            std::process::exit(3);
        };
        root_signature_checks();
        footprint_checks(&device);
        feature_checks(&device);
        resource_checks(&device);
        drop(device);
        drop(factory);
    }
    println!("INFO done: ok");
}

// ---- DXGI -------------------------------------------------------------

unsafe fn factory_checks() -> Option<IDXGIFactory4> {
    unsafe {
        let factory: windows::core::Result<IDXGIFactory4> =
            CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0));
        gold("CreateDXGIFactory2", hr(&factory));
        let factory = factory.ok()?;
        // QueryInterface chain: every version a caller might ask for.
        macro_rules! qi {
            ($($t:ty),*) => {$(
                let r = factory.cast::<$t>();
                gold(concat!("factory QI ", stringify!($t)), hr(&r));
            )*};
        }
        qi!(
            IDXGIObject,
            IDXGIFactory,
            IDXGIFactory1,
            IDXGIFactory2,
            IDXGIFactory3,
            IDXGIFactory4,
            IDXGIFactory5,
            IDXGIFactory6,
            IDXGIFactory7
        );
        let legacy: windows::core::Result<IDXGIFactory1> = CreateDXGIFactory1();
        gold("CreateDXGIFactory1", hr(&legacy));
        let plain: windows::core::Result<IDXGIFactory> = CreateDXGIFactory();
        gold("CreateDXGIFactory", hr(&plain));

        let mut index = 0;
        loop {
            let a = factory.EnumAdapters1(index);
            if index == 0 {
                gold("EnumAdapters1(0)", hr(&a));
            }
            let Ok(a) = a else {
                gold("EnumAdapters1 end", hr(&a));
                break;
            };
            let d = a.GetDesc1();
            if let Ok(d) = &d {
                info(
                    &format!("adapter {index}"),
                    format!(
                        "{} vendor={:#x} device={:#x} subsys={:#x} rev={} dedicated_video={} dedicated_system={} shared={} luid={:#x}:{:#x} flags={:#x}",
                        String::from_utf16_lossy(&d.Description).trim_end_matches('\0'),
                        d.VendorId,
                        d.DeviceId,
                        d.SubSysId,
                        d.Revision,
                        d.DedicatedVideoMemory,
                        d.DedicatedSystemMemory,
                        d.SharedSystemMemory,
                        d.AdapterLuid.HighPart,
                        d.AdapterLuid.LowPart,
                        d.Flags
                    ),
                );
                gold(
                    &format!("adapter {index} shape"),
                    format!(
                        "software={} dedicated_video_nonzero={} desc_hr={}",
                        d.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0,
                        d.DedicatedVideoMemory > 0,
                        hr(&d_ok(&d))
                    ),
                );
                let luid = d.AdapterLuid;
                let again = factory.EnumAdapterByLuid::<IDXGIAdapter1>(luid);
                gold(&format!("EnumAdapterByLuid {index}"), hr(&again));
            }
            // GetDesc through the older interface, and the adapter's QI chain.
            let a0 = a.cast::<IDXGIAdapter>();
            gold(&format!("adapter {index} QI IDXGIAdapter"), hr(&a0));
            let a3 = a.cast::<IDXGIAdapter3>();
            gold(&format!("adapter {index} QI IDXGIAdapter3"), hr(&a3));
            if let Ok(a3) = a3 {
                let mut m = DXGI_QUERY_VIDEO_MEMORY_INFO::default();
                let r = a3.QueryVideoMemoryInfo(0, DXGI_MEMORY_SEGMENT_GROUP_LOCAL, &mut m);
                gold(&format!("QueryVideoMemoryInfo {index}"), hr(&r));
                info(
                    "QueryVideoMemoryInfo",
                    format!("budget={} usage={}", m.Budget, m.CurrentUsage),
                );
            }
            index += 1;
            if index > 8 {
                break;
            }
        }
        let warp: windows::core::Result<IDXGIAdapter1> = factory.EnumWarpAdapter();
        gold("EnumWarpAdapter", hr(&warp));
        Some(factory)
    }
}

fn d_ok<T: Clone>(_: &T) -> windows::core::Result<()> {
    Ok(())
}

// ---- Device -----------------------------------------------------------

unsafe fn device_checks(factory: &IDXGIFactory4) -> Option<ID3D12Device> {
    unsafe {
        // The first adapter that is not software and has dedicated memory, as
        // victory-shader's `hardware_adapter` picks it.
        let mut chosen = None;
        for i in 0.. {
            let Ok(a) = factory.EnumAdapters1(i) else {
                break;
            };
            let d = a.GetDesc1().ok()?;
            if d.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 == 0 && d.DedicatedVideoMemory > 0 {
                chosen = Some(a);
                break;
            }
        }
        let Some(adapter) = chosen else {
            gold("hardware adapter", "none");
            return None;
        };
        let mut device: Option<ID3D12Device> = None;
        let r = D3D12CreateDevice(&adapter, D3D_FEATURE_LEVEL_12_0, &mut device);
        gold("D3D12CreateDevice(adapter, 12_0)", hr(&r));
        // victory-gpu passes no adapter.
        let mut none: Option<ID3D12Device> = None;
        let r2 = D3D12CreateDevice(None::<&IUnknown>, D3D_FEATURE_LEVEL_12_0, &mut none);
        gold("D3D12CreateDevice(null, 12_0)", hr(&r2));
        drop(none);
        // A query-only call (null out pointer) must succeed without making one.
        let r3 = D3D12CreateDevice(
            &adapter,
            D3D_FEATURE_LEVEL_12_0,
            std::ptr::null_mut::<Option<ID3D12Device>>(),
        );
        gold("D3D12CreateDevice(adapter, 12_0, null out)", hr(&r3));
        let device = device?;
        macro_rules! qi {
            ($($t:ty),*) => {$(
                let r = device.cast::<$t>();
                gold(concat!("device QI ", stringify!($t)), hr(&r));
            )*};
        }
        qi!(
            ID3D12Object,
            ID3D12DeviceChild,
            ID3D12Device,
            ID3D12Device1,
            ID3D12Device2,
            ID3D12Device3,
            ID3D12Device4,
            ID3D12Device5,
            ID3D12Device6,
            ID3D12Device7,
            ID3D12Device8,
            ID3D12Device9,
            ID3D12Device10,
            ID3D12Device11,
            ID3D12Device12,
            ID3D12Device13,
            ID3D12Device14
        );
        // Not a device interface: must be refused, not crash.
        let bogus = device.query(
            &windows::core::GUID::from_u128(0x12345678_1234_1234_1234_123456789abc),
            &mut std::ptr::null_mut(),
        );
        gold("device QI bogus", format!("{:#010x}", bogus.0 as u32));

        let luid = device.GetAdapterLuid();
        let a_luid = adapter.GetDesc1().ok()?.AdapterLuid;
        gold(
            "GetAdapterLuid == adapter luid",
            format!(
                "{}",
                luid.HighPart == a_luid.HighPart && luid.LowPart == a_luid.LowPart
            ),
        );
        gold("GetNodeCount", device.GetNodeCount().to_string());
        gold(
            "GetDeviceRemovedReason",
            hr(&device.GetDeviceRemovedReason()),
        );
        for (name, t) in [
            ("CBV_SRV_UAV", D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV),
            ("SAMPLER", D3D12_DESCRIPTOR_HEAP_TYPE_SAMPLER),
            ("RTV", D3D12_DESCRIPTOR_HEAP_TYPE_RTV),
            ("DSV", D3D12_DESCRIPTOR_HEAP_TYPE_DSV),
        ] {
            info(
                &format!("increment {name}"),
                device.GetDescriptorHandleIncrementSize(t).to_string(),
            );
        }
        Some(device)
    }
}

// ---- Root signatures --------------------------------------------------

fn blob_bytes(b: &ID3DBlob) -> Vec<u8> {
    // SAFETY: the blob owns the bytes while it lives.
    unsafe {
        std::slice::from_raw_parts(b.GetBufferPointer().cast::<u8>(), b.GetBufferSize()).to_vec()
    }
}

struct Cases {
    ranges10: Vec<D3D12_DESCRIPTOR_RANGE>,
    ranges11: Vec<D3D12_DESCRIPTOR_RANGE1>,
    samplers: Vec<D3D12_STATIC_SAMPLER_DESC>,
}

unsafe fn root_signature_checks() {
    unsafe {
        let cases = Cases {
            ranges10: vec![
                D3D12_DESCRIPTOR_RANGE {
                    RangeType: D3D12_DESCRIPTOR_RANGE_TYPE_SRV,
                    NumDescriptors: 2,
                    BaseShaderRegister: 0,
                    RegisterSpace: 0,
                    OffsetInDescriptorsFromTableStart: D3D12_DESCRIPTOR_RANGE_OFFSET_APPEND,
                },
                D3D12_DESCRIPTOR_RANGE {
                    RangeType: D3D12_DESCRIPTOR_RANGE_TYPE_UAV,
                    NumDescriptors: 4,
                    BaseShaderRegister: 1,
                    RegisterSpace: 3,
                    OffsetInDescriptorsFromTableStart: 8,
                },
                D3D12_DESCRIPTOR_RANGE {
                    RangeType: D3D12_DESCRIPTOR_RANGE_TYPE_SAMPLER,
                    NumDescriptors: u32::MAX,
                    BaseShaderRegister: 0,
                    RegisterSpace: 1,
                    OffsetInDescriptorsFromTableStart: D3D12_DESCRIPTOR_RANGE_OFFSET_APPEND,
                },
            ],
            ranges11: vec![
                D3D12_DESCRIPTOR_RANGE1 {
                    RangeType: D3D12_DESCRIPTOR_RANGE_TYPE_SRV,
                    NumDescriptors: 2,
                    BaseShaderRegister: 0,
                    RegisterSpace: 0,
                    Flags: D3D12_DESCRIPTOR_RANGE_FLAG_NONE,
                    OffsetInDescriptorsFromTableStart: D3D12_DESCRIPTOR_RANGE_OFFSET_APPEND,
                },
                D3D12_DESCRIPTOR_RANGE1 {
                    RangeType: D3D12_DESCRIPTOR_RANGE_TYPE_UAV,
                    NumDescriptors: 4,
                    BaseShaderRegister: 1,
                    RegisterSpace: 3,
                    Flags: D3D12_DESCRIPTOR_RANGE_FLAG_DESCRIPTORS_VOLATILE
                        | D3D12_DESCRIPTOR_RANGE_FLAG_DATA_VOLATILE,
                    OffsetInDescriptorsFromTableStart: 8,
                },
                D3D12_DESCRIPTOR_RANGE1 {
                    RangeType: D3D12_DESCRIPTOR_RANGE_TYPE_SAMPLER,
                    NumDescriptors: u32::MAX,
                    BaseShaderRegister: 0,
                    RegisterSpace: 1,
                    Flags: D3D12_DESCRIPTOR_RANGE_FLAG_DESCRIPTORS_VOLATILE,
                    OffsetInDescriptorsFromTableStart: D3D12_DESCRIPTOR_RANGE_OFFSET_APPEND,
                },
            ],
            samplers: vec![
                D3D12_STATIC_SAMPLER_DESC {
                    Filter: D3D12_FILTER_MIN_MAG_MIP_LINEAR,
                    AddressU: D3D12_TEXTURE_ADDRESS_MODE_WRAP,
                    AddressV: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
                    AddressW: D3D12_TEXTURE_ADDRESS_MODE_BORDER,
                    MipLODBias: 0.5,
                    MaxAnisotropy: 16,
                    ComparisonFunc: D3D12_COMPARISON_FUNC_LESS_EQUAL,
                    BorderColor: D3D12_STATIC_BORDER_COLOR_OPAQUE_WHITE,
                    MinLOD: 0.0,
                    MaxLOD: 12.0,
                    ShaderRegister: 2,
                    RegisterSpace: 5,
                    ShaderVisibility: D3D12_SHADER_VISIBILITY_PIXEL,
                },
                D3D12_STATIC_SAMPLER_DESC {
                    Filter: D3D12_FILTER_ANISOTROPIC,
                    AddressU: D3D12_TEXTURE_ADDRESS_MODE_MIRROR,
                    AddressV: D3D12_TEXTURE_ADDRESS_MODE_MIRROR_ONCE,
                    AddressW: D3D12_TEXTURE_ADDRESS_MODE_WRAP,
                    MipLODBias: -1.0,
                    MaxAnisotropy: 4,
                    ComparisonFunc: D3D12_COMPARISON_FUNC_NEVER,
                    BorderColor: D3D12_STATIC_BORDER_COLOR_TRANSPARENT_BLACK,
                    MinLOD: 1.0,
                    MaxLOD: f32::MAX,
                    ShaderRegister: 0,
                    RegisterSpace: 0,
                    ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
                },
            ],
        };
        let table10 = |r: &[D3D12_DESCRIPTOR_RANGE], vis| D3D12_ROOT_PARAMETER {
            ParameterType: D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE,
            Anonymous: D3D12_ROOT_PARAMETER_0 {
                DescriptorTable: D3D12_ROOT_DESCRIPTOR_TABLE {
                    NumDescriptorRanges: r.len() as u32,
                    pDescriptorRanges: r.as_ptr(),
                },
            },
            ShaderVisibility: vis,
        };
        let consts10 = |reg, space, n, vis| D3D12_ROOT_PARAMETER {
            ParameterType: D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS,
            Anonymous: D3D12_ROOT_PARAMETER_0 {
                Constants: D3D12_ROOT_CONSTANTS {
                    ShaderRegister: reg,
                    RegisterSpace: space,
                    Num32BitValues: n,
                },
            },
            ShaderVisibility: vis,
        };
        let view10 = |ty, reg, space, vis| D3D12_ROOT_PARAMETER {
            ParameterType: ty,
            Anonymous: D3D12_ROOT_PARAMETER_0 {
                Descriptor: D3D12_ROOT_DESCRIPTOR {
                    ShaderRegister: reg,
                    RegisterSpace: space,
                },
            },
            ShaderVisibility: vis,
        };
        let params10 = [
            table10(&cases.ranges10, D3D12_SHADER_VISIBILITY_ALL),
            consts10(0, 7, 4, D3D12_SHADER_VISIBILITY_ALL),
            view10(
                D3D12_ROOT_PARAMETER_TYPE_CBV,
                1,
                0,
                D3D12_SHADER_VISIBILITY_VERTEX,
            ),
            view10(
                D3D12_ROOT_PARAMETER_TYPE_SRV,
                2,
                9,
                D3D12_SHADER_VISIBILITY_ALL,
            ),
            view10(
                D3D12_ROOT_PARAMETER_TYPE_UAV,
                3,
                9,
                D3D12_SHADER_VISIBILITY_ALL,
            ),
        ];
        let params10b = [
            table10(&cases.ranges10[..2], D3D12_SHADER_VISIBILITY_PIXEL),
            consts10(3, 1, 2, D3D12_SHADER_VISIBILITY_GEOMETRY),
            table10(&cases.ranges10[1..], D3D12_SHADER_VISIBILITY_AMPLIFICATION),
            view10(
                D3D12_ROOT_PARAMETER_TYPE_UAV,
                5,
                5,
                D3D12_SHADER_VISIBILITY_MESH,
            ),
        ];
        let table11 = |r: &[D3D12_DESCRIPTOR_RANGE1], vis| D3D12_ROOT_PARAMETER1 {
            ParameterType: D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE,
            Anonymous: D3D12_ROOT_PARAMETER1_0 {
                DescriptorTable: D3D12_ROOT_DESCRIPTOR_TABLE1 {
                    NumDescriptorRanges: r.len() as u32,
                    pDescriptorRanges: r.as_ptr(),
                },
            },
            ShaderVisibility: vis,
        };
        let consts11 = |reg, space, n, vis| D3D12_ROOT_PARAMETER1 {
            ParameterType: D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS,
            Anonymous: D3D12_ROOT_PARAMETER1_0 {
                Constants: D3D12_ROOT_CONSTANTS {
                    ShaderRegister: reg,
                    RegisterSpace: space,
                    Num32BitValues: n,
                },
            },
            ShaderVisibility: vis,
        };
        let view11 = |ty, reg, space, flags, vis| D3D12_ROOT_PARAMETER1 {
            ParameterType: ty,
            Anonymous: D3D12_ROOT_PARAMETER1_0 {
                Descriptor: D3D12_ROOT_DESCRIPTOR1 {
                    ShaderRegister: reg,
                    RegisterSpace: space,
                    Flags: flags,
                },
            },
            ShaderVisibility: vis,
        };
        let params11 = [
            table11(&cases.ranges11, D3D12_SHADER_VISIBILITY_ALL),
            consts11(0, 7, 4, D3D12_SHADER_VISIBILITY_ALL),
            view11(
                D3D12_ROOT_PARAMETER_TYPE_CBV,
                1,
                0,
                D3D12_ROOT_DESCRIPTOR_FLAG_NONE,
                D3D12_SHADER_VISIBILITY_VERTEX,
            ),
            view11(
                D3D12_ROOT_PARAMETER_TYPE_SRV,
                2,
                9,
                D3D12_ROOT_DESCRIPTOR_FLAG_DATA_STATIC,
                D3D12_SHADER_VISIBILITY_ALL,
            ),
            view11(
                D3D12_ROOT_PARAMETER_TYPE_UAV,
                3,
                9,
                D3D12_ROOT_DESCRIPTOR_FLAG_DATA_VOLATILE,
                D3D12_SHADER_VISIBILITY_ALL,
            ),
        ];
        let params11b = [
            table11(&cases.ranges11[..2], D3D12_SHADER_VISIBILITY_PIXEL),
            consts11(3, 1, 2, D3D12_SHADER_VISIBILITY_GEOMETRY),
            table11(&cases.ranges11[1..], D3D12_SHADER_VISIBILITY_AMPLIFICATION),
            view11(
                D3D12_ROOT_PARAMETER_TYPE_UAV,
                5,
                5,
                D3D12_ROOT_DESCRIPTOR_FLAG_DATA_STATIC_WHILE_SET_AT_EXECUTE,
                D3D12_SHADER_VISIBILITY_MESH,
            ),
        ];
        // host.rs's shape: constants, root views, heap-indexed flags.
        let host11 = [
            consts11(0, 7, 8, D3D12_SHADER_VISIBILITY_ALL),
            view11(
                D3D12_ROOT_PARAMETER_TYPE_CBV,
                0,
                6,
                D3D12_ROOT_DESCRIPTOR_FLAG_NONE,
                D3D12_SHADER_VISIBILITY_ALL,
            ),
            view11(
                D3D12_ROOT_PARAMETER_TYPE_SRV,
                0,
                6,
                D3D12_ROOT_DESCRIPTOR_FLAG_NONE,
                D3D12_SHADER_VISIBILITY_ALL,
            ),
            view11(
                D3D12_ROOT_PARAMETER_TYPE_UAV,
                0,
                6,
                D3D12_ROOT_DESCRIPTOR_FLAG_NONE,
                D3D12_SHADER_VISIBILITY_ALL,
            ),
        ];
        let host_flags = D3D12_ROOT_SIGNATURE_FLAG_ALLOW_INPUT_ASSEMBLER_INPUT_LAYOUT
            | D3D12_ROOT_SIGNATURE_FLAG_CBV_SRV_UAV_HEAP_DIRECTLY_INDEXED
            | D3D12_ROOT_SIGNATURE_FLAG_SAMPLER_HEAP_DIRECTLY_INDEXED;

        let named10: Vec<(&str, D3D12_ROOT_SIGNATURE_DESC)> = vec![
            (
                "empty",
                D3D12_ROOT_SIGNATURE_DESC {
                    Flags: D3D12_ROOT_SIGNATURE_FLAG_ALLOW_INPUT_ASSEMBLER_INPUT_LAYOUT,
                    ..Default::default()
                },
            ),
            ("empty-none", D3D12_ROOT_SIGNATURE_DESC::default()),
            (
                "mixed",
                D3D12_ROOT_SIGNATURE_DESC {
                    NumParameters: params10.len() as u32,
                    pParameters: params10.as_ptr(),
                    NumStaticSamplers: cases.samplers.len() as u32,
                    pStaticSamplers: cases.samplers.as_ptr(),
                    Flags: D3D12_ROOT_SIGNATURE_FLAG_DENY_HULL_SHADER_ROOT_ACCESS,
                },
            ),
            (
                "two-tables",
                D3D12_ROOT_SIGNATURE_DESC {
                    NumParameters: params10b.len() as u32,
                    pParameters: params10b.as_ptr(),
                    Flags: D3D12_ROOT_SIGNATURE_FLAG_ALLOW_INPUT_ASSEMBLER_INPUT_LAYOUT
                        | D3D12_ROOT_SIGNATURE_FLAG_DENY_PIXEL_SHADER_ROOT_ACCESS,
                    ..Default::default()
                },
            ),
            (
                "constants-only",
                D3D12_ROOT_SIGNATURE_DESC {
                    NumParameters: 1,
                    pParameters: &raw const params10[1],
                    ..Default::default()
                },
            ),
            (
                "samplers-only",
                D3D12_ROOT_SIGNATURE_DESC {
                    NumStaticSamplers: 2,
                    pStaticSamplers: cases.samplers.as_ptr(),
                    ..Default::default()
                },
            ),
        ];
        for (name, d) in &named10 {
            for (ver_name, ver) in [
                ("1_0", D3D_ROOT_SIGNATURE_VERSION_1_0),
                ("1_1", D3D_ROOT_SIGNATURE_VERSION_1_1),
            ] {
                let (blob, hr_) = serialize(d, ver);
                gold(
                    &format!("serialize 1.0 desc {name} -> {ver_name}"),
                    format!("{hr_} {blob}"),
                );
            }
        }
        let named11: Vec<(&str, D3D12_ROOT_SIGNATURE_DESC1)> = vec![
            (
                "mixed",
                D3D12_ROOT_SIGNATURE_DESC1 {
                    NumParameters: params11.len() as u32,
                    pParameters: params11.as_ptr(),
                    NumStaticSamplers: cases.samplers.len() as u32,
                    pStaticSamplers: cases.samplers.as_ptr(),
                    Flags: D3D12_ROOT_SIGNATURE_FLAG_DENY_HULL_SHADER_ROOT_ACCESS,
                },
            ),
            (
                "two-tables",
                D3D12_ROOT_SIGNATURE_DESC1 {
                    NumParameters: params11b.len() as u32,
                    pParameters: params11b.as_ptr(),
                    NumStaticSamplers: 1,
                    pStaticSamplers: cases.samplers[1..].as_ptr(),
                    Flags: D3D12_ROOT_SIGNATURE_FLAG_LOCAL_ROOT_SIGNATURE,
                },
            ),
            (
                "host",
                D3D12_ROOT_SIGNATURE_DESC1 {
                    NumParameters: host11.len() as u32,
                    pParameters: host11.as_ptr(),
                    Flags: host_flags,
                    ..Default::default()
                },
            ),
            (
                "host-with-sampler",
                D3D12_ROOT_SIGNATURE_DESC1 {
                    NumParameters: host11.len() as u32,
                    pParameters: host11.as_ptr(),
                    NumStaticSamplers: 1,
                    pStaticSamplers: cases.samplers.as_ptr(),
                    Flags: host_flags,
                },
            ),
        ];
        let mut serialized: Vec<(String, Vec<u8>)> = Vec::new();
        for (name, d) in &named11 {
            let v = D3D12_VERSIONED_ROOT_SIGNATURE_DESC {
                Version: D3D_ROOT_SIGNATURE_VERSION_1_1,
                Anonymous: D3D12_VERSIONED_ROOT_SIGNATURE_DESC_0 { Desc_1_1: *d },
            };
            let (blob, hr_) = serialize_versioned(&v);
            gold(
                &format!("serialize versioned 1.1 desc {name}"),
                format!("{hr_} {}", blob.1),
            );
            if let Some(b) = blob.0 {
                serialized.push((format!("v11-{name}"), b));
            }
        }
        for (name, d) in &named10 {
            let v = D3D12_VERSIONED_ROOT_SIGNATURE_DESC {
                Version: D3D_ROOT_SIGNATURE_VERSION_1_0,
                Anonymous: D3D12_VERSIONED_ROOT_SIGNATURE_DESC_0 { Desc_1_0: *d },
            };
            let (blob, hr_) = serialize_versioned(&v);
            gold(
                &format!("serialize versioned 1.0 desc {name}"),
                format!("{hr_} {}", blob.1),
            );
            if let Some(b) = blob.0 {
                serialized.push((format!("v10-{name}"), b));
            }
        }
        // Rejected descriptions. D3DMetal crashes on some of these (it writes through the
        // null parameter array), so they run against our dlls only (`--ours`).
        if std::env::args().any(|a| a == "--ours") {
            let bad = D3D12_ROOT_SIGNATURE_DESC {
                NumParameters: 2,
                pParameters: std::ptr::null(),
                ..Default::default()
            };
            let (blob, hr_) = serialize(&bad, D3D_ROOT_SIGNATURE_VERSION_1_0);
            gold("serialize bad (null parameters)", format!("{hr_} {blob}"));
            let (blob, hr_) = serialize(&named10[0].1, D3D_ROOT_SIGNATURE_VERSION(7));
            gold("serialize bad version 7", format!("{hr_} {blob}"));
            let mut b: Option<ID3DBlob> = None;
            let r = D3D12SerializeRootSignature(
                std::ptr::null(),
                D3D_ROOT_SIGNATURE_VERSION_1_0,
                &mut b,
                None,
            );
            gold("serialize null desc", format!("{} {}", hr(&r), b.is_some()));
        }

        // Deserializers over what we serialized.
        for (name, bytes) in &serialized {
            let d = deserializer(bytes);
            let text = match &d {
                Ok(d) => describe10(&*d.GetRootSignatureDesc()),
                Err(e) => format!("{:#010x}", e.code().0 as u32),
            };
            gold(&format!("deserialize {name}"), text);
            let v = versioned_deserializer(bytes);
            match &v {
                Ok(v) => {
                    let native = v.GetRootSignatureDescAtVersion(D3D_ROOT_SIGNATURE_VERSION_1_1);
                    let t = match native {
                        Ok(p) => describe_versioned(&*p),
                        Err(e) => format!("{:#010x}", e.code().0 as u32),
                    };
                    gold(&format!("versioned deserialize {name} at 1.1"), t);
                    let at10 = v.GetRootSignatureDescAtVersion(D3D_ROOT_SIGNATURE_VERSION_1_0);
                    let t = match at10 {
                        Ok(p) => describe_versioned(&*p),
                        Err(e) => format!("{:#010x}", e.code().0 as u32),
                    };
                    gold(&format!("versioned deserialize {name} at 1.0"), t);
                    gold(
                        &format!("versioned deserialize {name} unconverted"),
                        describe_versioned(&*v.GetUnconvertedRootSignatureDesc()),
                    );
                }
                Err(e) => gold(
                    &format!("versioned deserialize {name}"),
                    format!("{:#010x}", e.code().0 as u32),
                ),
            }
        }
        // Garbage and truncated blobs.
        if let Some((_, bytes)) = serialized.first() {
            let t = deserializer(&bytes[..bytes.len() / 2]);
            info("deserialize truncated", hr(&t));
        }
        if std::env::args().any(|a| a == "--ours") {
            let t = deserializer(&[0xff; 64]);
            gold("deserialize garbage", format!("{}", t.is_err()));
            let t = deserializer(&[]);
            gold("deserialize empty", format!("{}", t.is_err()));
        }
    }
}

unsafe fn deserializer(bytes: &[u8]) -> windows::core::Result<ID3D12RootSignatureDeserializer> {
    unsafe {
        let mut out: *mut c_void = std::ptr::null_mut();
        D3D12CreateRootSignatureDeserializer(
            bytes.as_ptr().cast(),
            bytes.len(),
            &ID3D12RootSignatureDeserializer::IID,
            &mut out,
        )?;
        Ok(ID3D12RootSignatureDeserializer::from_raw(out))
    }
}

unsafe fn versioned_deserializer(
    bytes: &[u8],
) -> windows::core::Result<ID3D12VersionedRootSignatureDeserializer> {
    unsafe {
        let mut out: *mut c_void = std::ptr::null_mut();
        D3D12CreateVersionedRootSignatureDeserializer(
            bytes.as_ptr().cast(),
            bytes.len(),
            &ID3D12VersionedRootSignatureDeserializer::IID,
            &mut out,
        )?;
        Ok(ID3D12VersionedRootSignatureDeserializer::from_raw(out))
    }
}

unsafe fn serialize(
    d: &D3D12_ROOT_SIGNATURE_DESC,
    ver: D3D_ROOT_SIGNATURE_VERSION,
) -> (String, String) {
    unsafe {
        let mut blob: Option<ID3DBlob> = None;
        let mut err: Option<ID3DBlob> = None;
        let r = D3D12SerializeRootSignature(d, ver, &mut blob, Some(&mut err));
        let s = match &blob {
            Some(b) => hex(&blob_bytes(b)),
            None => "-".into(),
        };
        (format!("{} err={}", s, err.is_some()), hr(&r))
    }
}

unsafe fn serialize_versioned(
    d: &D3D12_VERSIONED_ROOT_SIGNATURE_DESC,
) -> ((Option<Vec<u8>>, String), String) {
    unsafe {
        let mut blob: Option<ID3DBlob> = None;
        let mut err: Option<ID3DBlob> = None;
        let r = D3D12SerializeVersionedRootSignature(d, &mut blob, Some(&mut err));
        let bytes = blob.as_ref().map(blob_bytes);
        let s = bytes.as_deref().map(hex).unwrap_or_else(|| "-".into());
        ((bytes, format!("{} err={}", s, err.is_some())), hr(&r))
    }
}

unsafe fn describe_params10(n: u32, p: *const D3D12_ROOT_PARAMETER) -> String {
    unsafe {
        let mut s = String::new();
        for i in 0..n as usize {
            let p = &*p.add(i);
            let _ = write!(
                s,
                "[type={} vis={} ",
                p.ParameterType.0, p.ShaderVisibility.0
            );
            match p.ParameterType {
                D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE => {
                    let t = p.Anonymous.DescriptorTable;
                    for j in 0..t.NumDescriptorRanges as usize {
                        let r = &*t.pDescriptorRanges.add(j);
                        let _ = write!(
                            s,
                            "(range type={} n={} base={} space={} off={})",
                            r.RangeType.0,
                            r.NumDescriptors,
                            r.BaseShaderRegister,
                            r.RegisterSpace,
                            r.OffsetInDescriptorsFromTableStart
                        );
                    }
                }
                D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS => {
                    let c = p.Anonymous.Constants;
                    let _ = write!(
                        s,
                        "const reg={} space={} n={}",
                        c.ShaderRegister, c.RegisterSpace, c.Num32BitValues
                    );
                }
                _ => {
                    let d = p.Anonymous.Descriptor;
                    let _ = write!(s, "desc reg={} space={}", d.ShaderRegister, d.RegisterSpace);
                }
            }
            s.push(']');
        }
        s
    }
}

unsafe fn describe_samplers(n: u32, p: *const D3D12_STATIC_SAMPLER_DESC) -> String {
    unsafe {
        let mut s = String::new();
        for i in 0..n as usize {
            let d = &*p.add(i);
            let _ = write!(
                s,
                "[filter={} addr={},{},{} bias={} aniso={} cmp={} border={} lod={}..{} reg={} space={} vis={}]",
                d.Filter.0,
                d.AddressU.0,
                d.AddressV.0,
                d.AddressW.0,
                d.MipLODBias,
                d.MaxAnisotropy,
                d.ComparisonFunc.0,
                d.BorderColor.0,
                d.MinLOD,
                d.MaxLOD,
                d.ShaderRegister,
                d.RegisterSpace,
                d.ShaderVisibility.0
            );
        }
        s
    }
}

unsafe fn describe10(d: &D3D12_ROOT_SIGNATURE_DESC) -> String {
    unsafe {
        format!(
            "1.0 params={} samplers={} flags={:#x} {} {}",
            d.NumParameters,
            d.NumStaticSamplers,
            d.Flags.0,
            describe_params10(d.NumParameters, d.pParameters),
            describe_samplers(d.NumStaticSamplers, d.pStaticSamplers)
        )
    }
}

unsafe fn describe11(d: &D3D12_ROOT_SIGNATURE_DESC1) -> String {
    unsafe {
        let mut s = String::new();
        for i in 0..d.NumParameters as usize {
            let p = &*d.pParameters.add(i);
            let _ = write!(
                s,
                "[type={} vis={} ",
                p.ParameterType.0, p.ShaderVisibility.0
            );
            match p.ParameterType {
                D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE => {
                    let t = p.Anonymous.DescriptorTable;
                    for j in 0..t.NumDescriptorRanges as usize {
                        let r = &*t.pDescriptorRanges.add(j);
                        let _ = write!(
                            s,
                            "(range type={} n={} base={} space={} flags={:#x} off={})",
                            r.RangeType.0,
                            r.NumDescriptors,
                            r.BaseShaderRegister,
                            r.RegisterSpace,
                            r.Flags.0,
                            r.OffsetInDescriptorsFromTableStart
                        );
                    }
                }
                D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS => {
                    let c = p.Anonymous.Constants;
                    let _ = write!(
                        s,
                        "const reg={} space={} n={}",
                        c.ShaderRegister, c.RegisterSpace, c.Num32BitValues
                    );
                }
                _ => {
                    let d = p.Anonymous.Descriptor;
                    let _ = write!(
                        s,
                        "desc reg={} space={} flags={:#x}",
                        d.ShaderRegister, d.RegisterSpace, d.Flags.0
                    );
                }
            }
            s.push(']');
        }
        format!(
            "1.1 params={} samplers={} flags={:#x} {} {}",
            d.NumParameters,
            d.NumStaticSamplers,
            d.Flags.0,
            s,
            describe_samplers(d.NumStaticSamplers, d.pStaticSamplers)
        )
    }
}

unsafe fn describe_versioned(v: &D3D12_VERSIONED_ROOT_SIGNATURE_DESC) -> String {
    unsafe {
        match v.Version {
            D3D_ROOT_SIGNATURE_VERSION_1_0 => describe10(&v.Anonymous.Desc_1_0),
            D3D_ROOT_SIGNATURE_VERSION_1_1 => describe11(&v.Anonymous.Desc_1_1),
            other => format!("version {}", other.0),
        }
    }
}

// ---- GetCopyableFootprints ----------------------------------------------

fn tex(
    dim: D3D12_RESOURCE_DIMENSION,
    w: u64,
    h: u32,
    depth: u16,
    mips: u16,
    format: DXGI_FORMAT,
) -> D3D12_RESOURCE_DESC {
    D3D12_RESOURCE_DESC {
        Dimension: dim,
        Alignment: 0,
        Width: w,
        Height: h,
        DepthOrArraySize: depth,
        MipLevels: mips,
        Format: format,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Layout: D3D12_TEXTURE_LAYOUT_UNKNOWN,
        Flags: D3D12_RESOURCE_FLAG_NONE,
    }
}

unsafe fn footprint_checks(device: &ID3D12Device) {
    unsafe {
        let buffer = |w: u64| D3D12_RESOURCE_DESC {
            Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
            ..tex(
                D3D12_RESOURCE_DIMENSION_BUFFER,
                w,
                1,
                1,
                1,
                DXGI_FORMAT_UNKNOWN,
            )
        };
        let t2 = D3D12_RESOURCE_DIMENSION_TEXTURE2D;
        let cases: Vec<(&str, D3D12_RESOURCE_DESC, u32, u32, u64)> = vec![
            ("buffer 1000", buffer(1000), 0, 1, 0),
            ("buffer 65536 base 512", buffer(65536), 0, 1, 512),
            (
                "rgba8 64x64 1 mip",
                tex(t2, 64, 64, 1, 1, DXGI_FORMAT_R8G8B8A8_UNORM),
                0,
                1,
                0,
            ),
            (
                "rgba8 4x4 1 mip",
                tex(t2, 4, 4, 1, 1, DXGI_FORMAT_R8G8B8A8_UNORM),
                0,
                1,
                0,
            ),
            (
                "rgba8 64x64 7 mips",
                tex(t2, 64, 64, 1, 7, DXGI_FORMAT_R8G8B8A8_UNORM),
                0,
                7,
                0,
            ),
            (
                "rgba8 100x37 6 mips x3 layers",
                tex(t2, 100, 37, 3, 6, DXGI_FORMAT_R8G8B8A8_UNORM),
                0,
                18,
                0,
            ),
            (
                "rgba8 100x37 6 mips x3 layers, subs 4..9 base 768",
                tex(t2, 100, 37, 3, 6, DXGI_FORMAT_R8G8B8A8_UNORM),
                4,
                5,
                768,
            ),
            (
                "rgba32f 33x5 3 mips",
                tex(t2, 33, 5, 1, 3, DXGI_FORMAT_R32G32B32A32_FLOAT),
                0,
                3,
                0,
            ),
            (
                "rgb32f 17x3",
                tex(t2, 17, 3, 1, 1, DXGI_FORMAT_R32G32B32_FLOAT),
                0,
                1,
                0,
            ),
            (
                "r8 5x5 3 mips",
                tex(t2, 5, 5, 1, 3, DXGI_FORMAT_R8_UNORM),
                0,
                3,
                0,
            ),
            (
                "r16 300x2",
                tex(t2, 300, 2, 1, 1, DXGI_FORMAT_R16_UINT),
                0,
                1,
                0,
            ),
            (
                "bc1 13x7 4 mips",
                tex(t2, 13, 7, 1, 4, DXGI_FORMAT_BC1_UNORM),
                0,
                4,
                0,
            ),
            (
                "bc7 64x64 7 mips",
                tex(t2, 64, 64, 1, 7, DXGI_FORMAT_BC7_UNORM),
                0,
                7,
                0,
            ),
            (
                "bc3 1x1",
                tex(t2, 1, 1, 1, 1, DXGI_FORMAT_BC3_UNORM),
                0,
                1,
                0,
            ),
            (
                "r8g8_b8g8 7x3",
                tex(t2, 7, 3, 1, 1, DXGI_FORMAT_R8G8_B8G8_UNORM),
                0,
                1,
                0,
            ),
            (
                "3d rgba8 16x16x8 3 mips",
                tex(
                    D3D12_RESOURCE_DIMENSION_TEXTURE3D,
                    16,
                    16,
                    8,
                    3,
                    DXGI_FORMAT_R8G8B8A8_UNORM,
                ),
                0,
                3,
                0,
            ),
            (
                "3d r32 5x6x7 3 mips",
                tex(
                    D3D12_RESOURCE_DIMENSION_TEXTURE3D,
                    5,
                    6,
                    7,
                    3,
                    DXGI_FORMAT_R32_FLOAT,
                ),
                0,
                3,
                0,
            ),
            (
                "1d rgba8 100 4 mips x2",
                tex(
                    D3D12_RESOURCE_DIMENSION_TEXTURE1D,
                    100,
                    1,
                    2,
                    4,
                    DXGI_FORMAT_R8G8B8A8_UNORM,
                ),
                0,
                8,
                0,
            ),
            (
                "d32 64x64",
                tex(t2, 64, 64, 1, 1, DXGI_FORMAT_D32_FLOAT),
                0,
                1,
                0,
            ),
            (
                "d32s8 33x17 2 mips (4 subs)",
                tex(t2, 33, 17, 1, 2, DXGI_FORMAT_D32_FLOAT_S8X24_UINT),
                0,
                4,
                0,
            ),
            (
                "d24s8 33x17 (2 subs)",
                tex(t2, 33, 17, 1, 1, DXGI_FORMAT_D24_UNORM_S8_UINT),
                0,
                2,
                0,
            ),
            (
                "nv12 64x48 (2 subs)",
                tex(t2, 64, 48, 1, 1, DXGI_FORMAT_NV12),
                0,
                2,
                0,
            ),
            (
                "r10g10b10a2 9x9 2 mips",
                tex(t2, 9, 9, 1, 2, DXGI_FORMAT_R10G10B10A2_UNORM),
                0,
                2,
                0,
            ),
        ];
        for (name, desc, first, n, base) in cases {
            let mut layouts = vec![D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default(); n as usize];
            let mut rows = vec![0u32; n as usize];
            let mut row_sizes = vec![0u64; n as usize];
            let mut total = 0u64;
            device.GetCopyableFootprints(
                &desc,
                first,
                n,
                base,
                Some(layouts.as_mut_ptr()),
                Some(rows.as_mut_ptr()),
                Some(row_sizes.as_mut_ptr()),
                Some(&mut total),
            );
            let mut s = String::new();
            for i in 0..n as usize {
                let f = &layouts[i].Footprint;
                let _ = write!(
                    s,
                    "[off={} fmt={} {}x{}x{} pitch={} rows={} rowsize={}]",
                    layouts[i].Offset,
                    f.Format.0,
                    f.Width,
                    f.Height,
                    f.Depth,
                    f.RowPitch,
                    rows[i],
                    row_sizes[i]
                );
            }
            // D3DMetal answers planar and packed formats in its own way (pitch 512 for
            // depth-stencil planes; garbage for R8G8_B8G8): shown, not compared.
            let quirky = ["d32s8", "d24s8", "nv12", "r8g8_b8g8"]
                .iter()
                .any(|p| name.starts_with(p));
            let text = format!("{s} total={total}");
            if quirky {
                info(&format!("footprints {name}"), text);
            } else {
                gold(&format!("footprints {name}"), text);
            }
        }
        // Sizes only (null out pointers).
        let mut total = 0u64;
        device.GetCopyableFootprints(
            &tex(t2, 64, 64, 1, 7, DXGI_FORMAT_R8G8B8A8_UNORM),
            0,
            7,
            0,
            None,
            None,
            None,
            Some(&mut total),
        );
        gold("footprints total only", total.to_string());
    }
}

// ---- CheckFeatureSupport --------------------------------------------------

unsafe fn feature_checks(device: &ID3D12Device) {
    unsafe {
        macro_rules! feature {
            ($id:expr, $ty:ty, $name:expr) => {{
                let mut data = std::mem::MaybeUninit::<$ty>::zeroed();
                let r = device.CheckFeatureSupport(
                    $id,
                    data.as_mut_ptr().cast(),
                    size_of::<$ty>() as u32,
                );
                let bytes =
                    std::slice::from_raw_parts(data.as_ptr().cast::<u8>(), size_of::<$ty>());
                info(
                    &format!("feature {}", $name),
                    format!("{} {}", hr(&r), hex(bytes)),
                );
            }};
        }
        feature!(
            D3D12_FEATURE_D3D12_OPTIONS,
            D3D12_FEATURE_DATA_D3D12_OPTIONS,
            "OPTIONS"
        );
        feature!(
            D3D12_FEATURE_ARCHITECTURE,
            D3D12_FEATURE_DATA_ARCHITECTURE,
            "ARCHITECTURE"
        );
        feature!(
            D3D12_FEATURE_GPU_VIRTUAL_ADDRESS_SUPPORT,
            D3D12_FEATURE_DATA_GPU_VIRTUAL_ADDRESS_SUPPORT,
            "GPU_VA"
        );
        feature!(
            D3D12_FEATURE_D3D12_OPTIONS1,
            D3D12_FEATURE_DATA_D3D12_OPTIONS1,
            "OPTIONS1"
        );
        feature!(
            D3D12_FEATURE_ROOT_SIGNATURE,
            D3D12_FEATURE_DATA_ROOT_SIGNATURE,
            "ROOT_SIGNATURE"
        );
        feature!(
            D3D12_FEATURE_ARCHITECTURE1,
            D3D12_FEATURE_DATA_ARCHITECTURE1,
            "ARCHITECTURE1"
        );
        feature!(
            D3D12_FEATURE_D3D12_OPTIONS2,
            D3D12_FEATURE_DATA_D3D12_OPTIONS2,
            "OPTIONS2"
        );
        feature!(
            D3D12_FEATURE_D3D12_OPTIONS3,
            D3D12_FEATURE_DATA_D3D12_OPTIONS3,
            "OPTIONS3"
        );
        feature!(
            D3D12_FEATURE_EXISTING_HEAPS,
            D3D12_FEATURE_DATA_EXISTING_HEAPS,
            "EXISTING_HEAPS"
        );
        feature!(
            D3D12_FEATURE_D3D12_OPTIONS4,
            D3D12_FEATURE_DATA_D3D12_OPTIONS4,
            "OPTIONS4"
        );
        feature!(
            D3D12_FEATURE_D3D12_OPTIONS5,
            D3D12_FEATURE_DATA_D3D12_OPTIONS5,
            "OPTIONS5"
        );
        feature!(
            D3D12_FEATURE_D3D12_OPTIONS6,
            D3D12_FEATURE_DATA_D3D12_OPTIONS6,
            "OPTIONS6"
        );
        feature!(
            D3D12_FEATURE_D3D12_OPTIONS7,
            D3D12_FEATURE_DATA_D3D12_OPTIONS7,
            "OPTIONS7"
        );
        feature!(
            D3D12_FEATURE_D3D12_OPTIONS8,
            D3D12_FEATURE_DATA_D3D12_OPTIONS8,
            "OPTIONS8"
        );
        feature!(
            D3D12_FEATURE_D3D12_OPTIONS9,
            D3D12_FEATURE_DATA_D3D12_OPTIONS9,
            "OPTIONS9"
        );
        feature!(
            D3D12_FEATURE_D3D12_OPTIONS10,
            D3D12_FEATURE_DATA_D3D12_OPTIONS10,
            "OPTIONS10"
        );
        feature!(
            D3D12_FEATURE_D3D12_OPTIONS11,
            D3D12_FEATURE_DATA_D3D12_OPTIONS11,
            "OPTIONS11"
        );
        feature!(
            D3D12_FEATURE_D3D12_OPTIONS12,
            D3D12_FEATURE_DATA_D3D12_OPTIONS12,
            "OPTIONS12"
        );
        feature!(
            D3D12_FEATURE_D3D12_OPTIONS13,
            D3D12_FEATURE_DATA_D3D12_OPTIONS13,
            "OPTIONS13"
        );
        feature!(
            D3D12_FEATURE_D3D12_OPTIONS14,
            D3D12_FEATURE_DATA_D3D12_OPTIONS14,
            "OPTIONS14"
        );
        feature!(
            D3D12_FEATURE_D3D12_OPTIONS15,
            D3D12_FEATURE_DATA_D3D12_OPTIONS15,
            "OPTIONS15"
        );
        feature!(
            D3D12_FEATURE_D3D12_OPTIONS16,
            D3D12_FEATURE_DATA_D3D12_OPTIONS16,
            "OPTIONS16"
        );
        feature!(
            D3D12_FEATURE_D3D12_OPTIONS17,
            D3D12_FEATURE_DATA_D3D12_OPTIONS17,
            "OPTIONS17"
        );
        feature!(
            D3D12_FEATURE_D3D12_OPTIONS18,
            D3D12_FEATURE_DATA_D3D12_OPTIONS18,
            "OPTIONS18"
        );
        feature!(
            D3D12_FEATURE_D3D12_OPTIONS19,
            D3D12_FEATURE_DATA_D3D12_OPTIONS19,
            "OPTIONS19"
        );
        feature!(
            D3D12_FEATURE_D3D12_OPTIONS20,
            D3D12_FEATURE_DATA_D3D12_OPTIONS20,
            "OPTIONS20"
        );
        feature!(
            D3D12_FEATURE_D3D12_OPTIONS21,
            D3D12_FEATURE_DATA_D3D12_OPTIONS21,
            "OPTIONS21"
        );

        // Shader models, highest first: what each device answers.
        for (name, sm) in [
            ("6_9", D3D_SHADER_MODEL(0x69)),
            ("6_8", D3D_SHADER_MODEL(0x68)),
            ("6_6", D3D_SHADER_MODEL_6_6),
        ] {
            let mut d = D3D12_FEATURE_DATA_SHADER_MODEL {
                HighestShaderModel: sm,
            };
            let r = device.CheckFeatureSupport(D3D12_FEATURE_SHADER_MODEL, (&raw mut d).cast(), 4);
            info(
                &format!("shader model asked {name}"),
                format!("{} highest={:#x}", hr(&r), d.HighestShaderModel.0),
            );
        }
        let mut sm = D3D12_FEATURE_DATA_SHADER_MODEL {
            HighestShaderModel: D3D_SHADER_MODEL_6_6,
        };
        let r = device.CheckFeatureSupport(D3D12_FEATURE_SHADER_MODEL, (&raw mut sm).cast(), 4);
        gold(
            "shader model >= 6.6",
            format!(
                "{} {}",
                hr(&r),
                sm.HighestShaderModel.0 >= D3D_SHADER_MODEL_6_6.0
            ),
        );
        let mut o1 = D3D12_FEATURE_DATA_D3D12_OPTIONS1::default();
        let r = device.CheckFeatureSupport(
            D3D12_FEATURE_D3D12_OPTIONS1,
            (&raw mut o1).cast(),
            size_of_val(&o1) as u32,
        );
        gold(
            "wave32 in range",
            format!(
                "{} wave_ops={} contains32={}",
                hr(&r),
                o1.WaveOps.as_bool(),
                (o1.WaveLaneCountMin..=o1.WaveLaneCountMax).contains(&32)
            ),
        );
        let mut o7 = D3D12_FEATURE_DATA_D3D12_OPTIONS7::default();
        let r = device.CheckFeatureSupport(
            D3D12_FEATURE_D3D12_OPTIONS7,
            (&raw mut o7).cast(),
            size_of_val(&o7) as u32,
        );
        info(
            "mesh shader tier",
            format!("{} {}", hr(&r), o7.MeshShaderTier.0),
        );
        // A wrong size and an unknown feature must fail, not crash.
        let mut sm = D3D12_FEATURE_DATA_SHADER_MODEL {
            HighestShaderModel: D3D_SHADER_MODEL_6_6,
        };
        let r = device.CheckFeatureSupport(D3D12_FEATURE_SHADER_MODEL, (&raw mut sm).cast(), 3);
        gold("CheckFeatureSupport wrong size", hr(&r));
        let r = device.CheckFeatureSupport(D3D12_FEATURE(9999), (&raw mut sm).cast(), 4);
        gold("CheckFeatureSupport unknown feature", hr(&r));
        // Format support: typed UAV load/store of R32_UINT and R32_FLOAT.
        for (name, f) in [
            ("R32_UINT", DXGI_FORMAT_R32_UINT),
            ("R32_FLOAT", DXGI_FORMAT_R32_FLOAT),
            ("R8G8B8A8_UNORM", DXGI_FORMAT_R8G8B8A8_UNORM),
        ] {
            let mut d = D3D12_FEATURE_DATA_FORMAT_SUPPORT {
                Format: f,
                ..Default::default()
            };
            let r = device.CheckFeatureSupport(
                D3D12_FEATURE_FORMAT_SUPPORT,
                (&raw mut d).cast(),
                size_of_val(&d) as u32,
            );
            info(
                &format!("format support {name}"),
                format!("{} s1={:#x} s2={:#x}", hr(&r), d.Support1.0, d.Support2.0),
            );
        }
    }
}

// ---- Resources, descriptors, a copy round trip ------------------------------

unsafe fn buffer_resource(
    device: &ID3D12Device,
    heap: D3D12_HEAP_TYPE,
    size: u64,
    flags: D3D12_RESOURCE_FLAGS,
    state: D3D12_RESOURCE_STATES,
) -> windows::core::Result<ID3D12Resource> {
    unsafe {
        let mut r: Option<ID3D12Resource> = None;
        device.CreateCommittedResource(
            &D3D12_HEAP_PROPERTIES {
                Type: heap,
                ..Default::default()
            },
            D3D12_HEAP_FLAG_NONE,
            &D3D12_RESOURCE_DESC {
                Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
                Flags: flags,
                ..tex(
                    D3D12_RESOURCE_DIMENSION_BUFFER,
                    size,
                    1,
                    1,
                    1,
                    DXGI_FORMAT_UNKNOWN,
                )
            },
            state,
            None,
            &mut r,
        )?;
        r.ok_or_else(|| windows::core::Error::from(E_FAIL))
    }
}

fn barrier(
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

unsafe fn resource_checks(device: &ID3D12Device) {
    unsafe {
        const N: u64 = 4096;
        let upload = buffer_resource(
            device,
            D3D12_HEAP_TYPE_UPLOAD,
            N,
            D3D12_RESOURCE_FLAG_NONE,
            D3D12_RESOURCE_STATE_GENERIC_READ,
        );
        gold("CreateCommittedResource upload buffer", hr(&upload));
        let default = buffer_resource(
            device,
            D3D12_HEAP_TYPE_DEFAULT,
            N,
            D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS,
            D3D12_RESOURCE_STATE_COMMON,
        );
        gold("CreateCommittedResource default buffer (UAV)", hr(&default));
        let readback = buffer_resource(
            device,
            D3D12_HEAP_TYPE_READBACK,
            N,
            D3D12_RESOURCE_FLAG_NONE,
            D3D12_RESOURCE_STATE_COPY_DEST,
        );
        gold("CreateCommittedResource readback buffer", hr(&readback));
        let (Ok(upload), Ok(default), Ok(readback)) = (upload, default, readback) else {
            return;
        };

        // Argument checks that a real runtime rejects.
        let bad = buffer_resource(
            device,
            D3D12_HEAP_TYPE_UPLOAD,
            N,
            D3D12_RESOURCE_FLAG_NONE,
            D3D12_RESOURCE_STATE_COPY_DEST,
        );
        gold("upload heap with COPY_DEST initial state", hr(&bad));
        let bad = buffer_resource(
            device,
            D3D12_HEAP_TYPE_READBACK,
            N,
            D3D12_RESOURCE_FLAG_NONE,
            D3D12_RESOURCE_STATE_GENERIC_READ,
        );
        gold("readback heap with GENERIC_READ initial state", hr(&bad));
        let bad = buffer_resource(
            device,
            D3D12_HEAP_TYPE_DEFAULT,
            0,
            D3D12_RESOURCE_FLAG_NONE,
            D3D12_RESOURCE_STATE_COMMON,
        );
        gold("zero-sized buffer", hr(&bad));

        for (name, r) in [
            ("upload", &upload),
            ("default", &default),
            ("readback", &readback),
        ] {
            let d = r.GetDesc();
            gold(
                &format!("GetDesc {name}"),
                format!(
                    "dim={} align={} w={} h={} depth={} mips={} fmt={} sample={}/{} layout={} flags={:#x}",
                    d.Dimension.0,
                    d.Alignment,
                    d.Width,
                    d.Height,
                    d.DepthOrArraySize,
                    d.MipLevels,
                    d.Format.0,
                    d.SampleDesc.Count,
                    d.SampleDesc.Quality,
                    d.Layout.0,
                    d.Flags.0
                ),
            );
            info(
                &format!("gpu va {name}"),
                format!("{:#x}", r.GetGPUVirtualAddress()),
            );
        }
        gold(
            "GetGPUVirtualAddress distinct and nonzero",
            format!(
                "{}",
                default.GetGPUVirtualAddress() != 0
                    && default.GetGPUVirtualAddress() != upload.GetGPUVirtualAddress()
                    && upload.GetGPUVirtualAddress() != readback.GetGPUVirtualAddress()
            ),
        );
        // Resource QI chain.
        gold(
            "resource QI ID3D12Pageable",
            hr(&default.cast::<ID3D12Pageable>()),
        );
        gold(
            "resource QI ID3D12DeviceChild",
            hr(&default.cast::<ID3D12DeviceChild>()),
        );
        gold(
            "resource QI ID3D12Object",
            hr(&default.cast::<ID3D12Object>()),
        );
        info(
            "resource QI ID3D12Resource1",
            hr(&default.cast::<ID3D12Resource1>()),
        );
        info(
            "resource QI ID3D12Resource2",
            hr(&default.cast::<ID3D12Resource2>()),
        );
        // GetDevice returns the creating device.
        let mut dev2: Option<ID3D12Device> = None;
        let r = default.GetDevice(&mut dev2);
        gold(
            "resource GetDevice",
            format!(
                "{} same={}",
                hr(&r),
                dev2.as_ref().is_some_and(|d| d.as_raw() == device.as_raw())
            ),
        );
        // Map / Unmap.
        let mut p: *mut c_void = std::ptr::null_mut();
        gold("Map upload", hr(&upload.Map(0, None, Some(&mut p))));
        for i in 0..N as usize {
            p.cast::<u8>().add(i).write((i * 7 + 3) as u8);
        }
        upload.Unmap(0, None);
        let mut q: *mut c_void = std::ptr::null_mut();
        let r = default.Map(0, None, Some(&mut q));
        gold("Map default (not CPU visible)", format!("{}", r.is_err()));

        // Descriptor heap: shader visible, UAV and CBV.
        let heap: windows::core::Result<ID3D12DescriptorHeap> =
            device.CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
                Type: D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV,
                NumDescriptors: 16,
                Flags: D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,
                NodeMask: 0,
            });
        gold("CreateDescriptorHeap CBV_SRV_UAV shader visible", hr(&heap));
        let heap = heap.ok();
        if let Some(heap) = &heap {
            let d = heap.GetDesc();
            gold(
                "heap GetDesc",
                format!(
                    "type={} n={} flags={} node={}",
                    d.Type.0, d.NumDescriptors, d.Flags.0, d.NodeMask
                ),
            );
            let inc = device
                .GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV)
                as usize;
            let cpu = heap.GetCPUDescriptorHandleForHeapStart();
            let gpu = heap.GetGPUDescriptorHandleForHeapStart();
            gold(
                "heap handles nonzero",
                format!("cpu={} gpu={}", cpu.ptr != 0, gpu.ptr != 0),
            );
            let at = D3D12_CPU_DESCRIPTOR_HANDLE {
                ptr: cpu.ptr + 3 * inc,
            };
            device.CreateUnorderedAccessView(
                &default,
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
                at,
            );
            device.CreateConstantBufferView(
                Some(&D3D12_CONSTANT_BUFFER_VIEW_DESC {
                    BufferLocation: upload.GetGPUVirtualAddress(),
                    SizeInBytes: 256,
                }),
                D3D12_CPU_DESCRIPTOR_HANDLE {
                    ptr: cpu.ptr + 4 * inc,
                },
            );
            device.CopyDescriptorsSimple(
                1,
                D3D12_CPU_DESCRIPTOR_HANDLE {
                    ptr: cpu.ptr + 5 * inc,
                },
                at,
                D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV,
            );
            gold("descriptor writes", "ok");
            let sampler_heap: windows::core::Result<ID3D12DescriptorHeap> = device
                .CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
                    Type: D3D12_DESCRIPTOR_HEAP_TYPE_SAMPLER,
                    NumDescriptors: 1,
                    Flags: D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,
                    NodeMask: 0,
                });
            gold(
                "CreateDescriptorHeap SAMPLER shader visible",
                hr(&sampler_heap),
            );
            let rtv: windows::core::Result<ID3D12DescriptorHeap> =
                device.CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
                    Type: D3D12_DESCRIPTOR_HEAP_TYPE_RTV,
                    NumDescriptors: 2,
                    Flags: D3D12_DESCRIPTOR_HEAP_FLAG_NONE,
                    NodeMask: 0,
                });
            gold("CreateDescriptorHeap RTV", hr(&rtv));
            let bad: windows::core::Result<ID3D12DescriptorHeap> =
                device.CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
                    Type: D3D12_DESCRIPTOR_HEAP_TYPE_RTV,
                    NumDescriptors: 2,
                    Flags: D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,
                    NodeMask: 0,
                });
            gold(
                "CreateDescriptorHeap RTV shader visible (invalid)",
                hr(&bad),
            );
        }

        // Root signature object from a serialized blob (compute-style).
        let root_params = [D3D12_ROOT_PARAMETER1 {
            ParameterType: D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS,
            Anonymous: D3D12_ROOT_PARAMETER1_0 {
                Constants: D3D12_ROOT_CONSTANTS {
                    ShaderRegister: 0,
                    RegisterSpace: 0,
                    Num32BitValues: 4,
                },
            },
            ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
        }];
        let v = D3D12_VERSIONED_ROOT_SIGNATURE_DESC {
            Version: D3D_ROOT_SIGNATURE_VERSION_1_1,
            Anonymous: D3D12_VERSIONED_ROOT_SIGNATURE_DESC_0 {
                Desc_1_1: D3D12_ROOT_SIGNATURE_DESC1 {
                    NumParameters: 1,
                    pParameters: root_params.as_ptr(),
                    ..Default::default()
                },
            },
        };
        let mut blob: Option<ID3DBlob> = None;
        let r = D3D12SerializeVersionedRootSignature(&v, &mut blob, None);
        gold("serialize for CreateRootSignature", hr(&r));
        if let Some(blob) = &blob {
            let root: windows::core::Result<ID3D12RootSignature> =
                device.CreateRootSignature(0, &blob_bytes(blob));
            gold("CreateRootSignature", hr(&root));
            if std::env::args().any(|a| a == "--ours") {
                let bad: windows::core::Result<ID3D12RootSignature> =
                    device.CreateRootSignature(0, &[1, 2, 3, 4]);
                gold("CreateRootSignature garbage", format!("{}", bad.is_err()));
            }
        }

        // Queue, allocator, list; copy upload -> default -> readback; fence.
        let queue: windows::core::Result<ID3D12CommandQueue> =
            device.CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC {
                Type: D3D12_COMMAND_LIST_TYPE_DIRECT,
                ..Default::default()
            });
        gold("CreateCommandQueue", hr(&queue));
        let Ok(queue) = queue else { return };
        let alloc: ID3D12CommandAllocator = device
            .CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT)
            .unwrap();
        let list: ID3D12GraphicsCommandList = device
            .CreateCommandList(
                0,
                D3D12_COMMAND_LIST_TYPE_DIRECT,
                &alloc,
                None::<&ID3D12PipelineState>,
            )
            .unwrap();
        gold(
            "list QI ID3D12CommandList",
            hr(&list.cast::<ID3D12CommandList>()),
        );
        gold(
            "list QI ID3D12GraphicsCommandList1",
            hr(&list.cast::<ID3D12GraphicsCommandList1>()),
        );
        info(
            "list QI ID3D12GraphicsCommandList6",
            hr(&list.cast::<ID3D12GraphicsCommandList6>()),
        );
        if let Some(heap) = &heap {
            list.SetDescriptorHeaps(&[Some(heap.clone())]);
        }
        list.ResourceBarrier(&[barrier(
            &default,
            D3D12_RESOURCE_STATE_COMMON,
            D3D12_RESOURCE_STATE_COPY_DEST,
        )]);
        list.CopyBufferRegion(&default, 0, &upload, 0, N);
        list.ResourceBarrier(&[barrier(
            &default,
            D3D12_RESOURCE_STATE_COPY_DEST,
            D3D12_RESOURCE_STATE_COPY_SOURCE,
        )]);
        list.CopyBufferRegion(&readback, 0, &default, 0, N);
        gold("list Close", hr(&list.Close()));
        gold("list Close twice", format!("{}", list.Close().is_err()));
        queue.ExecuteCommandLists(&[Some(list.cast().unwrap())]);
        let fence: ID3D12Fence = device.CreateFence(0, D3D12_FENCE_FLAG_NONE).unwrap();
        gold("Signal", hr(&queue.Signal(&fence, 1)));
        let event = CreateEventW(None, false, false, None).unwrap();
        gold(
            "SetEventOnCompletion",
            hr(&fence.SetEventOnCompletion(1, event)),
        );
        let wait = WaitForSingleObject(event, 10_000);
        gold(
            "fence wait",
            format!("{:?} completed={}", wait, fence.GetCompletedValue()),
        );
        let mut out: *mut c_void = std::ptr::null_mut();
        gold("Map readback", hr(&readback.Map(0, None, Some(&mut out))));
        let got = std::slice::from_raw_parts(out.cast::<u8>(), N as usize);
        let want: Vec<u8> = (0..N as usize).map(|i| (i * 7 + 3) as u8).collect();
        gold("copy round trip", format!("{}", got == want.as_slice()));
        readback.Unmap(0, None);
        gold(
            "GetDeviceRemovedReason after work",
            hr(&device.GetDeviceRemovedReason()),
        );
        // List reuse: Reset allocator and list.
        gold("allocator Reset", hr(&alloc.Reset()));
        gold(
            "list Reset",
            hr(&list.Reset(&alloc, None::<&ID3D12PipelineState>)),
        );
        gold("list Close after Reset", hr(&list.Close()));

        // Compute pipeline from junk must fail rather than crash.
        let params = [D3D12_ROOT_PARAMETER1 {
            ParameterType: D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS,
            Anonymous: D3D12_ROOT_PARAMETER1_0 {
                Constants: D3D12_ROOT_CONSTANTS {
                    ShaderRegister: 0,
                    RegisterSpace: 0,
                    Num32BitValues: 1,
                },
            },
            ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
        }];
        let v = D3D12_VERSIONED_ROOT_SIGNATURE_DESC {
            Version: D3D_ROOT_SIGNATURE_VERSION_1_1,
            Anonymous: D3D12_VERSIONED_ROOT_SIGNATURE_DESC_0 {
                Desc_1_1: D3D12_ROOT_SIGNATURE_DESC1 {
                    NumParameters: 1,
                    pParameters: params.as_ptr(),
                    ..Default::default()
                },
            },
        };
        let mut blob: Option<ID3DBlob> = None;
        let _ = D3D12SerializeVersionedRootSignature(&v, &mut blob, None);
        if let Some(blob) = blob {
            let root: ID3D12RootSignature =
                device.CreateRootSignature(0, &blob_bytes(&blob)).unwrap();
            if std::env::args().any(|a| a == "--ours") {
                let junk = [0u8; 64];
                let pso: windows::core::Result<ID3D12PipelineState> = device
                    .CreateComputePipelineState(&D3D12_COMPUTE_PIPELINE_STATE_DESC {
                        pRootSignature: std::mem::transmute_copy(&root),
                        CS: D3D12_SHADER_BYTECODE {
                            pShaderBytecode: junk.as_ptr().cast(),
                            BytecodeLength: junk.len(),
                        },
                        ..Default::default()
                    });
                gold(
                    "CreateComputePipelineState junk bytecode",
                    format!("{}", pso.is_err()),
                );
            }
            let _ = PCSTR::null();
        }
        // Everything is released by drop; a leak would show in the final line.
        drop((upload, default, readback, heap, queue, alloc, list, fence));
    }
}
