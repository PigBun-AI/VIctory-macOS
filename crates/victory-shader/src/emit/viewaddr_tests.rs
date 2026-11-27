//! The shadow-mask ray programs take the address of a buffer view they pick
//! from the view heap at an index they compute ([`super::shader::SLOT_VIEW_ADDRESSES`]).

use std::io::{Read, Seek, SeekFrom};

/// `shaders.rpf` offset, name and BLAKE3 prefix of the five programs.
const PROGRAMS: [(u64, &str, &str); 5] = [
    (0xd45e36f, "CS_TraceShadowRays_Eighth", "96ccb14ef981fd85"),
    (0xd464e6b, "CS_TraceShadowRays_Quarter", "0e64f14fae060d04"),
    (0xd46a9ee, "CS_TraceShadowRays_Half", "002e93a6cc2aa355"),
    (0xd46fb3c, "CS_TraceShadowRays_Full", "07c93bfb77fecebb"),
    (0xd474300, "CS_TraceShadowRays_FullAdaptive", "dbdbf36298727135"),
];

fn container(f: &mut std::fs::File, at: u64) -> Vec<u8> {
    f.seek(SeekFrom::Start(at)).unwrap();
    let mut h = [0; 12];
    f.read_exact(&mut h).unwrap();
    let u = |s: &[u8]| u32::from_le_bytes(s.try_into().unwrap()) as usize;
    assert_eq!(u(&h[0..4]), 0xddce7);
    let (len, n) = (u(&h[4..8]), u(&h[8..]));
    let mut packed = vec![0; len - 12];
    f.read_exact(&mut packed).unwrap();
    let mut bytes = vec![0; n + 1];
    let mut inflater = zlib_rs::Inflate::new(false, 15);
    assert_eq!(
        inflater.decompress(&packed, &mut bytes, zlib_rs::InflateFlush::NoFlush).unwrap(),
        zlib_rs::Status::StreamEnd
    );
    bytes.truncate(inflater.total_out() as usize);
    bytes
}

#[test]
#[ignore = "needs the game (VICTORY_GAME_DIR); run with --ignored"]
fn shadow_rays_translate_with_indexed_view_addresses() {
    let dir = std::env::var_os("VICTORY_GAME_DIR").expect("VICTORY_GAME_DIR: the game folder");
    let mut f = std::fs::File::open(std::path::Path::new(&dir).join("shaders.rpf")).unwrap();
    // SAFETY: set before any thread of this test reads it.
    unsafe {
        std::env::set_var("VICTORY_DESC_WHY", "1");
        std::env::set_var("VICTORY_VIEWADDR_TABLE", "1");
    }
    super::set_host_max_wave(32);
    super::shader::set_view_heap_limit(999_999);
    for (at, name, hash) in PROGRAMS {
        let bytes = container(&mut f, at);
        let _ = hash;
        let shader = super::translate(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
        let c = crate::Container::parse(&shader.dxil).unwrap();
        assert!(c.parts.iter().any(|p| p.fourcc.0 == *b"DXIL"), "{name}");
        // The root SRV of the address table is bound (`t251` or `t250`).
        let rts = c.parts.iter().find(|p| p.fourcc.0 == *b"RTS0");
        eprintln!("{name}: {} bytes of DXIL, RTS0 {}", shader.dxil.len(), rts.is_some());
    }
}