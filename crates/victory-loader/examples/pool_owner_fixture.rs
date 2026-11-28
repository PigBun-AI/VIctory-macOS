//! Disassembly-only copy, never executed. Same bytes/rel32 plan as runtime.
use std::{fs, io::Write};
use victory_loader::mac::pool_owner::{ASSEMBLY, GLOBAL, SITES, continuations, transfers};
fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() == 1 && args[0] == "--emit-asm" {
        print!("{ASSEMBLY}");
        return;
    }
    assert_eq!(args.len(), 3, "image, template COFF, new output");
    let mut data = fs::read(&args[0]).unwrap();
    let info = victory_pe::ImageInfo::parse(&data).unwrap();
    let obj = fs::read(&args[1]).unwrap();
    let u32at = |o| u32::from_le_bytes(obj[o..o + 4].try_into().unwrap()) as usize;
    assert_eq!(&obj[20..25], b".text");
    assert_eq!(&obj[52..54], &[0, 0], "template must be relocation free");
    let mut blob = obj[u32at(40)..u32at(40) + u32at(36)].to_vec();
    let base = 0x140000000usize;
    let [a, b, f] = continuations(base);
    for (i, v) in [
        base,
        base + GLOBAL,
        0,
        0,
        0,
        0,
        a,
        b,
        f,
        info.size_of_image as usize,
    ]
    .into_iter()
    .enumerate()
    {
        blob[i * 8..i * 8 + 8].copy_from_slice(&v.to_le_bytes());
    }
    let off = |r| info.rva_to_file_offset(r as u32).unwrap() as usize;
    let p = off(0x1000);
    data[p..p + blob.len()].copy_from_slice(&blob);
    for ((r, b, _), j) in SITES
        .into_iter()
        .zip(transfers(base as u64, base as u64 + 0x1000).unwrap())
    {
        let loc = off(r);
        assert_eq!(&data[loc..loc + b.len()], b);
        data[loc..loc + j.len()].copy_from_slice(&j)
    }
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&args[2])
        .unwrap()
        .write_all(&data)
        .unwrap();
}
