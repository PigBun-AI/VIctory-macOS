//! Make a DISASSEMBLY-ONLY copy. Off-line pool placed at +1000 in the copy's .text
//! (not an executable title fixture). Refuse existing output including the input.
use std::{fs, io::Write};
use victory_loader::mac::prestream::{DEST, SITES, matches, transfers};
fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() == 1 && args[0] == "--emit-asm" {
        print!("{}", victory_loader::mac::prestream::ASSEMBLY);
        return;
    }
    assert_eq!(args.len(), 3, "input, template COFF object, new output");
    let mut data = fs::read(&args[0]).unwrap();
    let info = victory_pe::ImageInfo::parse(&data).unwrap();
    let obj = fs::read(&args[1]).unwrap();
    let u32at = |o| u32::from_le_bytes(obj[o..o + 4].try_into().unwrap()) as usize;
    assert_eq!(&obj[20..25], b".text");
    assert_eq!(&obj[52..54], &[0, 0], "template must have no relocations");
    let start = u32at(40);
    let mut template = obj[start..start + u32at(36)].to_vec();
    let base = 0x140000000;
    let pool = 0x1000;
    for (i, r) in DEST.into_iter().enumerate() {
        template[(1 + i) * 8..(2 + i) * 8].copy_from_slice(&(base + r).to_le_bytes())
    }
    template[48..56].copy_from_slice(&base.to_le_bytes());
    let off = |r| info.rva_to_file_offset(r as u32).unwrap() as usize;
    let p = off(pool);
    data[p..p + template.len()].copy_from_slice(&template);
    for (i, (&(r, b), j)) in SITES
        .iter()
        .zip(transfers(base, base + pool as u64).unwrap())
        .enumerate()
    {
        let loc = off(r);
        assert!(matches(i, &data[loc..loc + b.len()]));
        data[loc..loc + b.len()].copy_from_slice(&j);
    }
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&args[2])
        .unwrap()
        .write_all(&data)
        .unwrap();
}
