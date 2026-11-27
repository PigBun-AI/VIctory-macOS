//! Which adapter the host device is on (docs/mac/PATCHES.md, G5). The Windows
//! line creates the device with `D3D12CreateDevice(None, ..)` and never says
//! which GPU that is; under Wine, D3DMetal may offer more than the Apple GPU
//! (an "AMD Compatibility Mode" adapter has been seen in test programs), and
//! timings from a wrong one mean nothing. Logged once at device creation.

#[cfg(windows)]
pub(crate) fn log(device: &windows::Win32::Graphics::Direct3D12::ID3D12Device) {
    use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory2, DXGI_CREATE_FACTORY_FLAGS, IDXGIAdapter1, IDXGIFactory4};
    // SAFETY: plain DXGI queries on the device's own LUID.
    unsafe {
        let Ok(factory) = CreateDXGIFactory2::<IDXGIFactory4>(DXGI_CREATE_FACTORY_FLAGS(0)) else { return };
        // every adapter DXGI offers, and which one the device is on
        let luid = device.GetAdapterLuid();
        let mut index = 0;
        while let Ok(adapter) = factory.EnumAdapters1(index) {
            if let Ok(desc) = adapter.GetDesc1() {
                let name = String::from_utf16_lossy(&desc.Description).trim_end_matches('\0').to_string();
                let selected = desc.AdapterLuid.LowPart == luid.LowPart && desc.AdapterLuid.HighPart == luid.HighPart;
                tracing::info!(
                    "DXGI adapter {index}: {name:?} vendor={:#x} device={:#x} flags={:#x} video memory {} MB shared {} MB{}",
                    desc.VendorId,
                    desc.DeviceId,
                    desc.Flags,
                    desc.DedicatedVideoMemory >> 20,
                    desc.SharedSystemMemory >> 20,
                    if selected { "  <- the host device is on this one" } else { "" }
                );
            }
            index += 1;
        }
        let _: Option<IDXGIAdapter1> = None;
    }
}
