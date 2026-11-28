//! Every point where memory is refused to the title, logged the same way
//! (mac H, matrix `mac.hle.low_address_commit`): the title writes 0 at a fixed
//! address and traps when it gets a NULL it does not expect (3 of 8 runs of
//! 2026-11-27 looked like that; the one examined was its own hang watchdog,
//! docs/mac/PATCHES.md G7), so a run says whether any allocation failed and
//! where, instead of the title's death being guessed at.
//!
//! One line per failure, `xmem alloc failed: site=<where> n=<count at this
//! site> size=<bytes> <detail>`: the first [`ALWAYS`] at each site, then
//! every power of two (a title that retries a refused request in a loop must
//! not flood the log); the count is exact. Nothing here depends on the host,
//! so the formatting and counting are tested natively; the callers are in
//! `xmem.rs` (Windows only).

use std::sync::atomic::{AtomicU64, Ordering};

/// Failures at one site that are always logged; later ones at powers of two.
pub const ALWAYS: u64 = 32;

/// The points of `xmem.rs` that hand the title a failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(usize)]
pub enum Site {
    /// `XMemVirtualAlloc`: the host refused, after the low-address retry.
    VirtualAlloc,
    /// `XMemAlloc`: the title's own routine in `__XMemAllocationHooks__` returned NULL.
    HookReturnedNull,
    /// `XMemAllocDefault` (`XMemAlloc` with no hook): its 32-bit path got NULL from `XMemVirtualAlloc`.
    Default32Bit,
    /// `XMemAllocDefault`: size and alignment that make no layout.
    DefaultBadLayout,
    /// `XMemAllocDefault`: the host heap is out of memory.
    DefaultHeapOom,
    /// `XMemAllocatePhysicalPages`: a null argument, or a size of 0 or that overflows.
    PhysicalPagesArgs,
    /// `XMemAllocatePhysicalPages`: the host would not create the section.
    PhysicalPagesSection,
    /// `XMemMapPhysicalPages`: a page number it never gave out.
    MapPagesUnknown,
    /// `XMemMapPhysicalPages`: the host would not map (or commit) the view.
    MapPagesHost,
    /// `XMemMapPhysicalPagesScatter`: no address list, or one page's mapping failed.
    MapScatter,
    /// Committing the section pages a view shows (a physical-page mapping).
    CommitPhysical,
}

/// How many sites there are.
const SITES: usize = 11;

impl Site {
    /// The name in the log line.
    pub const fn name(self) -> &'static str {
        match self {
            Site::VirtualAlloc => "XMemVirtualAlloc",
            Site::HookReturnedNull => "XMemAlloc.hook(__XMemAllocationHooks__)",
            Site::Default32Bit => "XMemAllocDefault.32bit",
            Site::DefaultBadLayout => "XMemAllocDefault.layout",
            Site::DefaultHeapOom => "XMemAllocDefault.heap",
            Site::PhysicalPagesArgs => "XMemAllocatePhysicalPages.args",
            Site::PhysicalPagesSection => "XMemAllocatePhysicalPages.section",
            Site::MapPagesUnknown => "XMemMapPhysicalPages.unknown_page",
            Site::MapPagesHost => "XMemMapPhysicalPages.host",
            Site::MapScatter => "XMemMapPhysicalPagesScatter",
            Site::CommitPhysical => "commit.physical_pages",
        }
    }

    /// Every site, in the order of the counters.
    pub const ALL: [Site; SITES] = [
        Site::VirtualAlloc,
        Site::HookReturnedNull,
        Site::Default32Bit,
        Site::DefaultBadLayout,
        Site::DefaultHeapOom,
        Site::PhysicalPagesArgs,
        Site::PhysicalPagesSection,
        Site::MapPagesUnknown,
        Site::MapPagesHost,
        Site::MapScatter,
        Site::CommitPhysical,
    ];
}

/// Failures counted per site.
pub struct Failures {
    counts: [AtomicU64; SITES],
}

impl Failures {
    pub const fn new() -> Self {
        Self { counts: [const { AtomicU64::new(0) }; SITES] }
    }

    /// Counts a failure at `site` and returns how many there have been there,
    /// this one included.
    pub fn count(&self, site: Site) -> u64 {
        self.counts[site as usize].fetch_add(1, Ordering::Relaxed) + 1
    }

    /// How many failures `site` has had.
    pub fn of(&self, site: Site) -> u64 {
        self.counts[site as usize].load(Ordering::Relaxed)
    }

    /// All failures so far, over every site.
    pub fn total(&self) -> u64 {
        Site::ALL.iter().map(|&s| self.of(s)).sum()
    }
}

impl Default for Failures {
    fn default() -> Self {
        Self::new()
    }
}

/// Whether the `n`th failure at a site is logged: the first [`ALWAYS`], then the powers of two.
pub fn logged(n: u64) -> bool {
    n <= ALWAYS || n.is_power_of_two()
}

/// The log line for the `n`th failure at `site`.
pub fn line(site: Site, n: u64, size: usize, detail: &str) -> String {
    format!(
        "xmem alloc failed: site={} n={n} size={size:#x} ({} MiB){}{detail}",
        site.name(),
        size >> 20,
        if detail.is_empty() { "" } else { " " }
    )
}

/// The title's allocation attributes (`XMemAlloc`'s second argument) in words: the 32-bit flag
/// (bit 33) and the alignment as a power of two (5 bits from bit 24; less than 16 bytes is 16).
pub fn attributes(attributes: u64) -> String {
    format!(
        "attributes={attributes:#x} ({}, align {})",
        if attributes & (1 << 33) != 0 { "32-bit address" } else { "any address" },
        1usize << (attributes >> 24 & 0x1f).max(4)
    )
}

static FAILURES: Failures = Failures::new();

/// The process's failure counts.
pub fn failures() -> &'static Failures {
    &FAILURES
}

/// Counts a failure at `site` in `failures` and logs it (a warning) if it is one that is logged;
/// returns the count at the site.
pub fn note_in(failures: &Failures, site: Site, size: usize, detail: &str) -> u64 {
    let n = failures.count(site);
    if logged(n) {
        tracing::warn!("{}", line(site, n, size, detail));
    }
    n
}

/// Counts and logs a failure of the title's memory at `site` ([`note_in`] on the process's counts).
pub fn note(site: Site, size: usize, detail: &str) -> u64 {
    note_in(&FAILURES, site, size, detail)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failure_line_says_where_how_often_and_how_big() {
        assert_eq!(
            line(Site::VirtualAlloc, 1, 0x1000_0000, "kind=0x3000"),
            "xmem alloc failed: site=XMemVirtualAlloc n=1 size=0x10000000 (256 MiB) kind=0x3000"
        );
        assert_eq!(
            line(Site::HookReturnedNull, 3, 64, ""),
            "xmem alloc failed: site=XMemAlloc.hook(__XMemAllocationHooks__) n=3 size=0x40 (0 MiB)"
        );
    }

    #[test]
    fn the_hook_result_is_a_failure_point_with_its_attributes() {
        let f = Failures::new();
        // The title's routine returned NULL for a 32-bit request aligned to 64 KiB (2^16).
        let attrs = (1u64 << 33) | (16 << 24);
        assert_eq!(f.count(Site::HookReturnedNull), 1);
        assert_eq!(attributes(attrs), "attributes=0x210000000 (32-bit address, align 65536)");
        assert_eq!(attributes(0), "attributes=0x0 (any address, align 16)");
        assert_eq!(f.of(Site::HookReturnedNull), 1);
    }

    #[test]
    fn every_site_counts_by_itself_and_all_have_a_distinct_name() {
        let f = Failures::new();
        for (i, site) in Site::ALL.into_iter().enumerate() {
            assert_eq!(site as usize, i, "ALL is in the order of the counters");
            assert_eq!(f.of(site), 0);
        }
        assert_eq!(f.count(Site::VirtualAlloc), 1);
        assert_eq!(f.count(Site::VirtualAlloc), 2);
        assert_eq!(f.count(Site::DefaultHeapOom), 1);
        assert_eq!((f.of(Site::VirtualAlloc), f.of(Site::DefaultHeapOom), f.of(Site::MapScatter)), (2, 1, 0));
        assert_eq!(f.total(), 3);
        let mut names: Vec<_> = Site::ALL.iter().map(|s| s.name()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), SITES);
    }

    #[test]
    fn the_first_failures_are_all_logged_then_the_powers_of_two() {
        assert!((1..=ALWAYS).all(logged));
        assert!(!logged(ALWAYS + 1) && !logged(100) && logged(64) && logged(128) && !logged(129));
        // The count stays exact whatever is logged.
        let f = Failures::new();
        let last = (0..1000).map(|_| note_in(&f, Site::Default32Bit, 8, "")).last();
        assert_eq!(last, Some(1000));
        assert_eq!(f.of(Site::Default32Bit), 1000);
    }
}
