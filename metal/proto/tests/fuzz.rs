//! Random and mutated input must never panic, and whatever is accepted must
//! survive a second round trip. No external crates: a small xorshift.

mod common;

use common::*;
use victory_metal_proto::file::FileReader;
use victory_metal_proto::root::RootSigDesc;
use victory_metal_proto::stream::{Decoder, STREAM_HEADER_SIZE};
use victory_metal_proto::{Caps, Command, Encoder};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// Decodes `bytes`; if accepted, re-encodes and decodes again and compares.
fn exercise(bytes: &[u8]) -> bool {
    let Ok(dec) = Decoder::new(bytes) else {
        return false;
    };
    let caps = dec.header().caps;
    let Ok(cmds) = dec.collect_all() else {
        return false;
    };
    let mut e = Encoder::new(caps);
    for c in &cmds {
        e.push(c).expect("an accepted command encodes");
    }
    let again = e.finish();
    let cmds2: Vec<Command<'_>> = Decoder::new(&again).unwrap().collect_all().unwrap();
    assert_eq!(cmds, cmds2);
    true
}

#[test]
fn random_bytes_never_panic() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    for i in 0..20_000 {
        let len = rng.below(200);
        let mut b: Vec<u8> = (0..len).map(|_| rng.next() as u8).collect();
        // Half the time, give it a plausible header so the commands get exercised.
        if i % 2 == 0 && len >= STREAM_HEADER_SIZE {
            let proper = synthetic_compute();
            b[..STREAM_HEADER_SIZE].copy_from_slice(&proper[..STREAM_HEADER_SIZE]);
            let total = (len as u32) & !7;
            b.truncate(total as usize);
            if b.len() >= STREAM_HEADER_SIZE {
                let n = b.len() as u32;
                b[24..28].copy_from_slice(&n.to_le_bytes());
                let count = rng.below(6) as u32;
                b[28..32].copy_from_slice(&count.to_le_bytes());
            }
        }
        let _ = exercise(&b);
        let _ = RootSigDesc::decode(&b);
        if let Ok(r) = FileReader::new(&b) {
            for blk in r {
                let _ = blk;
            }
        }
    }
}

#[test]
fn mutated_valid_streams_never_panic() {
    let mut rng = Rng(0x1234_5678_9ABC_DEF1);
    let seeds = [
        synthetic_compute(),
        encode(&one_of_each()),
        synthetic_dump(),
    ];
    let mut accepted = 0;
    for round in 0..40_000 {
        let mut b = seeds[round % seeds.len()].clone();
        for _ in 0..1 + rng.below(4) {
            if b.is_empty() {
                break;
            }
            match rng.below(5) {
                0 => {
                    let i = rng.below(b.len());
                    let bit = rng.below(8);
                    b[i] ^= 1 << bit;
                }
                1 => {
                    let i = rng.below(b.len());
                    let v = rng.next() as u8;
                    b[i] = v;
                }
                2 => {
                    let n = rng.below(b.len() + 1);
                    b.truncate(n);
                }
                3 => {
                    let n = rng.below(16);
                    for _ in 0..n {
                        b.push(rng.next() as u8);
                    }
                }
                _ => {
                    // Patch a length-looking word with an extreme value.
                    if b.len() >= 8 {
                        let i = rng.below(b.len() - 4);
                        let pick = rng.below(7);
                        let v = [0u32, 1, 7, 8, 0x7FFF_FFFF, 0xFFFF_FFF8, u32::MAX][pick];
                        b[i..i + 4].copy_from_slice(&v.to_le_bytes());
                    }
                }
            }
        }
        if FileReader::looks_like_file(&b) {
            if let Ok(r) = FileReader::new(&b) {
                for blk in r.flatten() {
                    if exercise(blk) {
                        accepted += 1;
                    }
                }
            }
        } else if exercise(&b) {
            accepted += 1;
        }
        let _ = RootSigDesc::decode(&b);
    }
    // Some mutations (padding bytes, buffer contents) are harmless; make sure the
    // loop really reached the accept path as well as the refuse path.
    assert!(accepted > 100, "only {accepted} accepted");
}

#[test]
fn random_command_sequences_round_trip() {
    let mut rng = Rng(42);
    let pool = one_of_each();
    for _ in 0..2_000 {
        let n = rng.below(12);
        let cmds: Vec<Command<'_>> = (0..n).map(|_| pool[rng.below(pool.len())]).collect();
        let block = encode(&cmds);
        assert_eq!(Decoder::new(&block).unwrap().collect_all().unwrap(), cmds);
    }
    let _ = Caps::NONE;
}
