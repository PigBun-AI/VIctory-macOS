//! The adapter the front presents: what DXGI `GetDesc1` reports.
//!
//! Placeholder values until the back reports the real GPU (name from the
//! Metal device, memory from `recommendedMaxWorkingSetSize`).

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdapterInfo {
    pub name: String,
    pub vendor_id: u32,
    pub device_id: u32,
    pub sub_sys_id: u32,
    pub revision: u32,
    /// `DedicatedVideoMemory`. Victory-shader's `hardware_adapter` insists on
    /// a non-zero value, and D3DMetal reports the working-set size here too.
    pub dedicated_video_memory: u64,
    pub dedicated_system_memory: u64,
    pub shared_system_memory: u64,
    pub luid_low: u32,
    pub luid_high: i32,
    /// `DXGI_ADAPTER_FLAG_SOFTWARE` would make the callers skip it; never set.
    pub software: bool,
}

/// Apple's PCI-style vendor id (what Metal tooling reports for Apple GPUs).
pub const VENDOR_APPLE: u32 = 0x106B;

impl AdapterInfo {
    /// A stand-in: 24 GiB, a fixed LUID.
    pub fn placeholder() -> AdapterInfo {
        let mem = 24u64 << 30;
        AdapterInfo {
            name: "Apple GPU (VIctory Metal)".to_string(),
            vendor_id: VENDOR_APPLE,
            device_id: 0,
            sub_sys_id: 0,
            revision: 0,
            dedicated_video_memory: mem,
            dedicated_system_memory: 0,
            shared_system_memory: mem,
            luid_low: 0x4d54_4c31,
            luid_high: 0,
            software: false,
        }
    }

    /// The name as the fixed 128-element UTF-16 array of `DXGI_ADAPTER_DESC1`.
    pub fn description_utf16(&self) -> [u16; 128] {
        let mut out = [0u16; 128];
        for (o, c) in out.iter_mut().take(127).zip(self.name.encode_utf16()) {
            *o = c;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn description_is_nul_terminated_and_truncated() {
        let mut a = AdapterInfo::placeholder();
        let d = a.description_utf16();
        assert_eq!(String::from_utf16(&d[..d.iter().position(|c| *c == 0).unwrap()]).unwrap(), a.name);
        a.name = "x".repeat(300);
        let d = a.description_utf16();
        assert_eq!(d[126], b'x' as u16);
        assert_eq!(d[127], 0);
    }

    #[test]
    fn placeholder_passes_the_callers_adapter_filter() {
        let a = AdapterInfo::placeholder();
        assert!(!a.software && a.dedicated_video_memory > 0);
    }
}
