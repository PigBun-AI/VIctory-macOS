//! shared.root.signature (docs/mac/PATCHES.md G12): the console root signature serializer against the D3D12 one.
//!
//! The console's `D3D12SerializeRootSignature` takes the PC description plus console-only flag bits (the top byte of
//! `D3D12_ROOT_SIGNATURE_FLAGS`); the host's must see the PC description alone. Reference: the D3D12 serializer itself
//! (`D3D12SerializeRootSignature`, the Windows SDK's definition of the blob), compared byte for byte: for every
//! combination of the PC flags (bits 0-10, `D3D12_ROOT_SIGNATURE_FLAG_*` of `d3d12.h`) and each console flag pattern,
//! our blob is the host's blob of the description without the console bits (or both fail, with the same result), for
//! a description with every kind of root parameter and a static sampler.
use super::*;
use crate::object::slot;
use windows::Win32::Graphics::Direct3D12::*;

fn description(flags: u32, with: impl FnOnce(&Desc) -> R) -> R {
    let ranges = [
        D3D12_DESCRIPTOR_RANGE { RangeType: D3D12_DESCRIPTOR_RANGE_TYPE_SRV, NumDescriptors: 4, BaseShaderRegister: 0, RegisterSpace: 0, OffsetInDescriptorsFromTableStart: 0 },
        D3D12_DESCRIPTOR_RANGE { RangeType: D3D12_DESCRIPTOR_RANGE_TYPE_UAV, NumDescriptors: u32::MAX, BaseShaderRegister: 0, RegisterSpace: 1, OffsetInDescriptorsFromTableStart: 4 },
    ];
    let parameters = [
        D3D12_ROOT_PARAMETER {
            ParameterType: D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS,
            Anonymous: D3D12_ROOT_PARAMETER_0 { Constants: D3D12_ROOT_CONSTANTS { ShaderRegister: 0, RegisterSpace: 0, Num32BitValues: 4 } },
            ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
        },
        D3D12_ROOT_PARAMETER {
            ParameterType: D3D12_ROOT_PARAMETER_TYPE_CBV,
            Anonymous: D3D12_ROOT_PARAMETER_0 { Descriptor: D3D12_ROOT_DESCRIPTOR { ShaderRegister: 1, RegisterSpace: 0 } },
            ShaderVisibility: D3D12_SHADER_VISIBILITY_PIXEL,
        },
        D3D12_ROOT_PARAMETER {
            ParameterType: D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE,
            Anonymous: D3D12_ROOT_PARAMETER_0 { DescriptorTable: D3D12_ROOT_DESCRIPTOR_TABLE { NumDescriptorRanges: 2, pDescriptorRanges: ranges.as_ptr() } },
            ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
        },
    ];
    let sampler = D3D12_STATIC_SAMPLER_DESC {
        Filter: D3D12_FILTER_ANISOTROPIC,
        AddressU: D3D12_TEXTURE_ADDRESS_MODE_WRAP,
        AddressV: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
        AddressW: D3D12_TEXTURE_ADDRESS_MODE_MIRROR,
        MipLODBias: 0.5,
        MaxAnisotropy: 8,
        ComparisonFunc: D3D12_COMPARISON_FUNC_LESS_EQUAL,
        BorderColor: D3D12_STATIC_BORDER_COLOR_OPAQUE_WHITE,
        MinLOD: 0.0,
        MaxLOD: 12.0,
        ShaderRegister: 2,
        RegisterSpace: 0,
        ShaderVisibility: D3D12_SHADER_VISIBILITY_PIXEL,
    };
    let desc = Desc {
        parameter_count: 3,
        parameters: parameters.as_ptr().cast(),
        sampler_count: 1,
        samplers: (&raw const sampler).cast(),
        flags,
    };
    with(&desc)
}

type R = (HRESULT, Vec<u8>);

fn blob_bytes(blob: *mut c_void) -> Vec<u8> {
    if blob.is_null() {
        return Vec::new();
    }
    let blob = blob.cast_const().cast::<Object>();
    let data = slot::<extern "system" fn(This) -> *const u8>(blob, 0x18)(blob);
    let size = slot::<extern "system" fn(This) -> usize>(blob, 0x20)(blob);
    // SAFETY: the blob's own buffer.
    let bytes = unsafe { std::slice::from_raw_parts(data, size) }.to_vec();
    slot::<extern "system" fn(This) -> u32>(blob, 0x10)(blob);
    bytes
}

fn ours(flags: u32) -> R {
    description(flags, |d| {
        let (mut blob, mut error) = (std::ptr::null_mut(), std::ptr::null_mut());
        let hr = serialize(d, 1, &mut blob, &mut error);
        let _ = blob_bytes(error);
        (hr, blob_bytes(blob))
    })
}

fn host(flags: u32) -> R {
    description(flags, |d| {
        let (mut blob, mut error) = (std::ptr::null_mut(), std::ptr::null_mut());
        let address = host_export(&OnceLock::new(), c"D3D12SerializeRootSignature").expect("the host's d3d12.dll");
        // SAFETY: the export of this signature, a description we built.
        let hr = unsafe { std::mem::transmute::<usize, Serialize>(address)(d, 1, &mut blob, &mut error) };
        let _ = blob_bytes(error);
        (hr, blob_bytes(blob))
    })
}

#[test]
fn console_flags_never_reach_the_host_serializer_and_the_rest_do() {
    let mut served = 0;
    for low in 0..0x800u32 {
        let want = host(low);
        for console in [0u32, 0x0100_0000, 0x0200_0000, 0x8000_0000, 0xff00_0000] {
            assert_eq!(ours(low | console), want, "flags {low:#x} + console {console:#x}");
        }
        served += usize::from(want.0 == 0);
    }
    // the table is not trivially all failures: the host takes many of the combinations
    assert!(served > 100, "{served} combinations served");
}
