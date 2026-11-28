//! shared.device.feature_query (docs/mac/PATCHES.md G12): `ID3D12Device::CheckFeatureSupport` as the console device
//! answers it, against the D3D12 headers' definitions and against the host device the work is really done by.
//!
//! Reference: `d3d12.h` (Windows SDK 10.0.26100): `D3D12_FEATURE_DATA_D3D12_OPTIONS` (15 fields, the console's
//! has one more at index 11: `device.rs` OPTIONS), `D3D12_FEATURE_DATA_ARCHITECTURE`, `D3D12_FEATURE_DATA_FEATURE_LEVELS`,
//! `D3D12_FEATURE_DATA_MULTISAMPLE_QUALITY_LEVELS`; enums: `D3D12_SHADER_MIN_PRECISION_SUPPORT` (NONE 0, 10_BIT 1, 16_BIT 2),
//! `D3D12_TILED_RESOURCES_TIER` 0-3, `D3D12_RESOURCE_BINDING_TIER` 1-3, `D3D12_CONSERVATIVE_RASTERIZATION_TIER` 0-3,
//! `D3D12_CROSS_NODE_SHARING_TIER` 0-3, `D3D12_RESOURCE_HEAP_TIER` 1-2.
use std::ffi::c_void;

use crate::object::{E_INVALIDARG, HRESULT, S_OK, This, slot};

type Check = extern "system" fn(This, u32, *mut c_void, u32) -> HRESULT;

fn ask(feature: u32, data: &mut [u32]) -> HRESULT {
    let device = crate::device::device();
    slot::<Check>(device, 0x68)(device, feature, data.as_mut_ptr().cast(), (data.len() * 4) as u32)
}

#[test]
fn options_are_values_the_headers_define() {
    let mut o = [0xdead_beefu32; 16];
    assert_eq!(ask(0, &mut o), S_OK);
    for (i, name) in [(0, "double ops"), (1, "logic ops"), (5, "ps stencil ref"), (6, "typed uav loads"), (7, "rovs"), (10, "standard swizzle"), (13, "cross adapter row major"), (14, "vp/rt index")] {
        assert!(o[i] <= 1, "{name} is a BOOL");
    }
    assert!(matches!(o[2], 0..=3), "min precision is a bit set of 10_BIT, 16_BIT");
    assert!(o[3] <= 3 && o[8] <= 3 && o[12] <= 3, "tiers 0-3");
    assert!((1..=3).contains(&o[4]), "resource binding tier 1-3");
    assert!((1..=2).contains(&o[15]), "resource heap tier 1-2");
    assert!((32..=64).contains(&o[9]), "GPU virtual address bits per resource");
    // the size is the console's: the PC's (15 fields) and others are refused, a null pointer too
    assert_eq!(ask(0, &mut [0u32; 15]), E_INVALIDARG);
    assert_eq!(ask(0, &mut [0u32; 17]), E_INVALIDARG);
    let device = crate::device::device();
    assert_eq!(slot::<Check>(device, 0x68)(device, 0, std::ptr::null_mut(), 64), E_INVALIDARG);
}

/// What the console device claims must be what the host device can do (the title is told it has these features, and
/// the translated shaders and the views use them); where it claims more, this lists it.
#[cfg(windows)]
#[test]
fn options_do_not_claim_more_than_the_host_device_has() {
    use windows::Win32::Graphics::Direct3D12::*;
    let Some(host) = crate::host::host() else { return };
    let mut h = [0u32; 15];
    // SAFETY: the structure is 15 dwords.
    unsafe { host.device.CheckFeatureSupport(D3D12_FEATURE_D3D12_OPTIONS, h.as_mut_ptr().cast(), 60) }.expect("host options");
    let mut o = [0u32; 16];
    assert_eq!(ask(0, &mut o), S_OK);
    // the console's field 11 has no PC counterpart
    let ours: Vec<u32> = o.iter().enumerate().filter(|&(i, _)| i != 11).map(|(_, &v)| v).collect();
    let names = ["double", "logic ops", "min precision", "tiled", "binding", "ps stencil ref", "typed uav", "rov", "conservative", "va bits", "std swizzle", "cross node", "cross adapter", "vp/rt index", "heap tier"];
    let more: Vec<String> = (0..15).filter(|&i| ours[i] > h[i] && i != 9).map(|i| format!("{}: console says {}, host {}", names[i], ours[i], h[i])).collect();
    eprintln!("MACPROVE host options {h:?}\nMACPROVE console options {ours:?}\nMACPROVE claims more: {more:?}");
    if std::env::var_os("VICTORY_PROVE_STRICT_OPTIONS").is_some() {
        assert!(more.is_empty(), "{more:?}");
    }
}

#[test]
fn architecture_levels_and_samples() {
    let mut a = [0u32, 9, 9, 9];
    assert_eq!(ask(1, &mut a), S_OK);
    assert_eq!(&a[1..], &[0, 1, 1], "not tile based, UMA, cache coherent UMA");
    assert_eq!(ask(1, &mut [1u32, 0, 0, 0]), E_INVALIDARG, "one node");
    // multisample quality levels: [format, sample count, flags, out count]
    for (count, levels) in [(1u32, 1u32), (2, 1), (4, 1), (8, 1), (3, 0), (16, 0)] {
        let mut q = [28u32, count, 0, 9]; // DXGI_FORMAT_R8G8B8A8_UNORM
        assert_eq!(ask(4, &mut q), S_OK);
        assert_eq!(q[3], levels, "{count} samples");
    }
}
