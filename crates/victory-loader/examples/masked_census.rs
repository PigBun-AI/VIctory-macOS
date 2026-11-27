//! Static census of the masked-store patch (L1/L6) against a title image:
//! `cargo run --release -p victory-loader --example masked_census -- <exe or cached image> [site-list.txt ...]`.
//!
//! Prints what the loader's scan finds (`mac::masked::masked_stores`), whether every
//! site decodes and gets a trampoline, the bytes in front of each site (a prefix there
//! would make the trampoline's `lea` wrong), and, for each census list given (hex RVAs,
//! one per line, as `research/census/title-isa-*/isa-sites-*.txt`), how many of its
//! sites the scan covers and which it does not.
use std::collections::BTreeSet;

use victory_loader::mac::masked::{self, MaskedKind};

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("usage: masked_census <image> [list ...]");
    let bytes = std::fs::read(&path).expect("read image");
    let info = victory_pe::ImageInfo::parse(&bytes).expect("parse PE");
    let found = masked::masked_stores(&info, &bytes);
    println!("scan: {}", masked::summary(&found));
    let rvas: BTreeSet<u32> = found.iter().map(|f| f.rva).collect();
    let audit = masked::audit(&info, &bytes);
    println!(
        "audit: {} undecoded, {} without a trampoline, {} with a segment/67 byte right before the VEX prefix",
        audit.undecoded.len(),
        audit.no_trampoline.len(),
        audit.prefixed.len()
    );
    for (rva, b) in audit.prefixed.iter().take(10) {
        println!("  prefix-like byte {b:#04x} before {rva:#x}");
    }
    let kind = |k: MaskedKind| rvas_of(&found, k);
    println!(
        "kinds: masked {} vcvtps2ph {} vextract {}",
        kind(MaskedKind::Vpmaskmovd).len()
            + kind(MaskedKind::Vpmaskmovq).len()
            + kind(MaskedKind::Vmaskmovps).len()
            + kind(MaskedKind::Vmaskmovpd).len(),
        kind(MaskedKind::Vcvtps2ph).len(),
        kind(MaskedKind::Vextractf128).len() + kind(MaskedKind::Vextracti128).len()
    );
    for list in args {
        let text = std::fs::read_to_string(&list).expect("read list");
        let sites: BTreeSet<u32> = text
            .lines()
            .filter_map(|l| u32::from_str_radix(l.trim().trim_start_matches("0x"), 16).ok())
            .collect();
        let missed: Vec<_> = sites.difference(&rvas).collect();
        println!(
            "{list}: {} sites, {} covered by the scan, {} missed{}",
            sites.len(),
            sites.len() - missed.len(),
            missed.len(),
            if missed.is_empty() {
                String::new()
            } else {
                format!(": {:x?}", &missed[..missed.len().min(20)])
            }
        );
        let extra: Vec<_> = rvas.difference(&sites).collect();
        println!("  scan sites not in this list: {}", extra.len());
    }
}

fn rvas_of(found: &[masked::MaskedStore], k: MaskedKind) -> Vec<u32> {
    found.iter().filter(|f| f.kind == k).map(|f| f.rva).collect()
}
