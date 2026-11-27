//! L6's scan against a title image and the static census:
//! `cargo run --release -p victory-loader --example cmpxchg16_census -- <image> [cmpxchg16-sites.tsv]`
//! (the tsv `tools/mac/census-cmpxchg.py.txt` writes: first column hex RVA of the instruction's first
//! prefix byte). Prints how many sites the scan finds and, with the tsv, how many agree.
use std::collections::BTreeSet;

use victory_loader::mac::cmpxchg16;

fn main() {
    let mut args = std::env::args().skip(1);
    let bytes = std::fs::read(args.next().expect("usage: cmpxchg16_census <image> [tsv]")).expect("read image");
    let info = victory_pe::ImageInfo::parse(&bytes).expect("parse PE");
    let found: BTreeSet<u32> = cmpxchg16::scan(&info, &bytes).iter().map(|s| s.rva).collect();
    println!("scan: {} sites", found.len());
    if let Some(tsv) = args.next() {
        let want: BTreeSet<u32> = std::fs::read_to_string(tsv)
            .expect("read tsv")
            .lines()
            .skip(1)
            .filter_map(|l| u32::from_str_radix(l.split('\t').next()?, 16).ok())
            .collect();
        println!(
            "census: {} sites; scan misses {}, scan has {} more",
            want.len(),
            want.difference(&found).count(),
            found.difference(&want).count()
        );
    }
}
