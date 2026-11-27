//! What a reader must refuse: every way a stream, a file or a description can
//! be wrong ends in an `Error`, never a panic and never an accepted block.

mod common;

use common::*;
use victory_metal_proto::cmd::*;
use victory_metal_proto::file::{FILE_HEADER_SIZE, FileReader, FileWriter};
use victory_metal_proto::status::{Error, Status};
use victory_metal_proto::stream::{Decoder, STREAM_HEADER_SIZE};
use victory_metal_proto::types::*;
use victory_metal_proto::{Command, PROTO_VERSION};

fn first_error(block: &[u8]) -> Error {
    match Decoder::new(block) {
        Err(e) => e,
        Ok(d) => d.collect_all().expect_err("block should be refused"),
    }
}

fn dispatch_payload() -> Vec<u8> {
    let mut p = Vec::new();
    for v in [1u32, 1, 1, 0] {
        p.extend_from_slice(&v.to_le_bytes());
    }
    p
}

#[test]
fn unknown_opcode() {
    for code in [
        0x0000u16, 0x0006, 0x00FF, 0x0106, 0x0213, 0x0301, 0x0A00, 0x0F02, 0x8000, 0xFFFF,
    ] {
        let block = block_with(1, &raw_cmd(code, 1, 0, 0, &[]));
        assert_eq!(first_error(&block), Error::UnknownOpcode(code), "{code:#x}");
    }
}

#[test]
fn reserved_opcode_ranges_are_named_as_such() {
    for (first, last, _) in RESERVED_RANGES {
        for code in [*first, *last] {
            let block = block_with(1, &raw_cmd(code, 1, 0, 0, &[]));
            let e = first_error(&block);
            assert_eq!(e, Error::ReservedOpcode(code));
            assert_eq!(e.status(), Status::UNSUPPORTED);
        }
    }
}

#[test]
fn unknown_version() {
    for v in [0u8, 2, 255] {
        let block = block_with(1, &raw_cmd(0x0200, v, 0, 16, &dispatch_payload()));
        assert_eq!(
            first_error(&block),
            Error::UnknownOpcodeVersion {
                opcode: 0x0200,
                version: v
            }
        );
        assert_eq!(first_error(&block).status(), Status::BAD_VERSION);
    }
}

#[test]
fn nonzero_command_flags() {
    let block = block_with(1, &raw_cmd(0x0200, 1, 1, 16, &dispatch_payload()));
    assert!(matches!(
        first_error(&block),
        Error::BadValue { opcode: 0x0200, .. }
    ));
}

#[test]
fn a_good_dispatch_by_hand_is_accepted() {
    // The helpers build what they claim to, so the refusals below mean something.
    let block = block_with(1, &raw_cmd(0x0200, 1, 0, 16, &dispatch_payload()));
    let cmds = Decoder::new(&block).unwrap().collect_all().unwrap();
    assert_eq!(
        cmds,
        vec![Command::Dispatch(Dispatch {
            x: 1,
            y: 1,
            z: 1,
            _pad: 0
        })]
    );
}

#[test]
fn length_out_of_bounds() {
    // Longer than the block.
    let block = block_with(1, &raw_cmd(0x0200, 1, 0, 24, &dispatch_payload()));
    assert!(matches!(
        first_error(&block),
        Error::BadPayloadLength {
            opcode: 0x0200,
            len: 24
        }
    ));
    // Absurdly long.
    let block = block_with(1, &raw_cmd(0x0200, 1, 0, 0xFFFF_FFF8, &dispatch_payload()));
    assert!(matches!(
        first_error(&block),
        Error::BadPayloadLength { .. }
    ));
    let block = block_with(1, &raw_cmd(0x0F00, 1, 0, 0x7FFF_FFF8, &[0; 16]));
    assert!(matches!(
        first_error(&block),
        Error::BadPayloadLength { .. }
    ));
    // Not a multiple of 8.
    let block = block_with(1, &raw_cmd(0x0200, 1, 0, 12, &dispatch_payload()));
    assert!(matches!(
        first_error(&block),
        Error::BadPayloadLength { .. }
    ));
    // Shorter or longer than the opcode's fixed payload.
    let block = block_with(1, &raw_cmd(0x0200, 1, 0, 8, &dispatch_payload()[..8]));
    assert!(matches!(
        first_error(&block),
        Error::BadPayloadLength { .. }
    ));
    let mut long = dispatch_payload();
    long.extend_from_slice(&[0; 8]);
    let block = block_with(1, &raw_cmd(0x0200, 1, 0, 24, &long));
    assert!(matches!(
        first_error(&block),
        Error::BadPayloadLength { .. }
    ));
    // A header with no room for its payload at all.
    let block = block_with(1, &raw_cmd(0x0200, 1, 0, 16, &[]));
    assert!(matches!(
        first_error(&block),
        Error::BadPayloadLength { .. }
    ));
}

#[test]
fn a_block_that_is_not_a_multiple_of_eight_is_refused() {
    for n in 1..8 {
        let block = block_with(1, &raw_cmd(0x0200, 1, 0, 16, &[])[..n]);
        assert_eq!(first_error(&block), Error::BadHeader, "{n}");
    }
}

#[test]
fn variable_payload_lengths_must_be_honest() {
    // DEBUG_LABEL says 100 bytes but the payload is 16.
    let mut p = Vec::new();
    p.extend_from_slice(&100u32.to_le_bytes());
    p.extend_from_slice(&0u32.to_le_bytes());
    p.extend_from_slice(&[b'x'; 8]);
    let block = block_with(1, &raw_cmd(0x0005, 1, 0, 16, &p));
    assert!(matches!(
        first_error(&block),
        Error::BadPayloadLength { opcode: 5, .. }
    ));
    // A label that fits but is padded to the wrong size.
    let mut p = Vec::new();
    p.extend_from_slice(&3u32.to_le_bytes());
    p.extend_from_slice(&0u32.to_le_bytes());
    p.extend_from_slice(&[b'x'; 16]);
    let block = block_with(1, &raw_cmd(0x0005, 1, 0, 24, &p));
    assert!(matches!(
        first_error(&block),
        Error::BadPayloadLength { opcode: 5, .. }
    ));
    // SET_ROOT_CONSTANTS with a count that is huge (would overflow naive maths).
    let mut p = Vec::new();
    for v in [0u32, 0, 0, u32::MAX] {
        p.extend_from_slice(&v.to_le_bytes());
    }
    let block = block_with(1, &raw_cmd(0x0103, 1, 0, 16, &p));
    assert!(matches!(
        first_error(&block),
        Error::BadPayloadLength { opcode: 0x103, .. }
    ));
    // ... and one that is within the payload but over the limit of 64 values.
    let mut p = Vec::new();
    for v in [0u32, 0, 0, 65] {
        p.extend_from_slice(&v.to_le_bytes());
    }
    p.extend_from_slice(&[0; 65 * 4 + 4]);
    let block = block_with(1, &raw_cmd(0x0103, 1, 0, p.len() as u32, &p));
    assert!(matches!(
        first_error(&block),
        Error::BadValue { opcode: 0x103, .. }
    ));
    // BUFFER_DATA claiming 2^63 bytes.
    let mut p = Vec::new();
    for v in [1u32, 0, 0, 0] {
        p.extend_from_slice(&v.to_le_bytes());
    }
    p.extend_from_slice(&(1u64 << 63).to_le_bytes());
    let block = block_with(1, &raw_cmd(0x0F01, 1, 0, 24, &p));
    assert!(matches!(
        first_error(&block),
        Error::BadPayloadLength { .. }
    ));
    // SYNC_RECORD whose parts do not add up.
    let mut p = Vec::new();
    for v in [1u32, 0, 24, 8] {
        p.extend_from_slice(&v.to_le_bytes());
    }
    p.extend_from_slice(&[0; 8]);
    let block = block_with(1, &raw_cmd(0x0F00, 1, 0, 24, &p));
    assert!(matches!(
        first_error(&block),
        Error::BadPayloadLength { .. }
    ));
}

#[test]
fn bad_values_inside_a_good_frame() {
    // Barrier with a stage bit that does not exist.
    let c = Command::Barrier(Barrier {
        before: stage::ALL,
        after: stage::ALL,
        ..Barrier::default()
    });
    let mut block = encode(&[c]);
    // before := 0x0100, at payload offset 2.
    block[STREAM_HEADER_SIZE + 8 + 2..STREAM_HEADER_SIZE + 8 + 4]
        .copy_from_slice(&0x0100u16.to_le_bytes());
    assert!(matches!(
        first_error(&block),
        Error::BadValue {
            opcode: 0x300,
            field: "stages"
        }
    ));
    // Unknown barrier class, unknown visibility bit, empty memory stages.
    for (field, mutate) in [
        (
            "class",
            Barrier {
                class: BarrierClass(9),
                ..Barrier::memory(1, 1)
            },
        ),
        (
            "visibility",
            Barrier {
                visibility: 4,
                ..Barrier::memory(1, 1)
            },
        ),
        ("stages", Barrier::memory(0, 1)),
        ("stages", Barrier::memory(1, 0)),
    ] {
        let mut e = victory_metal_proto::Encoder::new(Caps::NONE);
        assert!(
            e.push(&Command::Barrier(mutate)).is_err(),
            "{field}: encoder"
        );
        // Write the same thing by hand and check the decoder agrees.
        let mut body = Vec::new();
        victory_metal_proto::wire::Wire::put(&mutate, &mut body);
        let block = block_with(1, &raw_cmd(0x0300, 1, 0, 16, &body));
        assert!(
            matches!(first_error(&block), Error::BadValue { opcode: 0x300, field: f } if f == field),
            "{field}"
        );
    }
    // An aliasing barrier with no stages is fine.
    assert!(
        Barrier {
            ..Barrier::aliasing(Default::default(), Default::default())
        }
        .after
            == stage::ALL
    );
    // Unknown bind point.
    let c = Command::SetRootSignature(SetRootSignature {
        bind_point: BindPoint(9),
        root_sig: Default::default(),
    });
    assert!(
        victory_metal_proto::Encoder::new(Caps::NONE)
            .push(&c)
            .is_err()
    );
    // A root descriptor of kind TABLE.
    let c = Command::SetRootDescriptor(SetRootDescriptor {
        kind: RootParamKind::TABLE,
        ..Default::default()
    });
    assert!(
        victory_metal_proto::Encoder::new(Caps::NONE)
            .push(&c)
            .is_err()
    );
    // A label over the limit.
    let long = vec![b'a'; MAX_LABEL + 1];
    assert!(
        victory_metal_proto::Encoder::new(Caps::NONE)
            .push(&Command::DebugLabel(&long))
            .is_err()
    );
}

#[test]
fn execute_indirect_rules_are_enforced() {
    use victory_metal_proto::ids::*;
    use victory_metal_proto::indirect::*;
    use victory_metal_proto::wire::Wire;
    let sig = |args: &[IndirectArg]| {
        let mut v = Vec::new();
        for a in args {
            Wire::put(a, &mut v);
        }
        v
    };
    let arg = |kind, index, dest_offset, count| IndirectArg {
        kind,
        index,
        dest_offset,
        count,
    };
    let dispatch = arg(IndirectArgKind::DISPATCH, 0, 0, 0);
    let constants = arg(IndirectArgKind::CONSTANT, 1, 0, 2);
    let cmd = |signature: &[u8], stride, root_sig| {
        let c = Command::ExecuteIndirect(ExecuteIndirect {
            args: ResId(1),
            args_offset: 0,
            count: ResId::NULL,
            count_offset: 0,
            max_count: 4,
            stride,
            root_sig,
            signature,
        });
        victory_metal_proto::Encoder::new(Caps::NONE).push(&c)
    };
    // Good: the root signature is there when a root argument is.
    assert!(cmd(&sig(&[dispatch]), 12, RootSigId::NULL).is_ok());
    assert!(cmd(&sig(&[constants, dispatch]), 20, RootSigId(1)).is_ok());
    // Root arguments without a root signature, and the other way round.
    assert!(cmd(&sig(&[constants, dispatch]), 20, RootSigId::NULL).is_err());
    assert!(cmd(&sig(&[dispatch]), 12, RootSigId(1)).is_err());
    // The action is last and alone; there is one.
    assert!(cmd(&sig(&[dispatch, constants]), 20, RootSigId(1)).is_err());
    assert!(cmd(&sig(&[dispatch, dispatch]), 24, RootSigId::NULL).is_err());
    assert!(cmd(&sig(&[constants]), 8, RootSigId(1)).is_err());
    assert!(cmd(&[], 12, RootSigId::NULL).is_err());
    // A stride that does not hold the record, or is not whole words.
    assert!(cmd(&sig(&[constants, dispatch]), 16, RootSigId(1)).is_err());
    assert!(cmd(&sig(&[dispatch]), 14, RootSigId::NULL).is_err());
    // Kinds this proto does not know; the graphics ones are known.
    assert!(
        cmd(
            &sig(&[arg(IndirectArgKind(77), 0, 0, 0)]),
            16,
            RootSigId::NULL
        )
        .is_err()
    );
    assert!(
        cmd(
            &sig(&[arg(IndirectArgKind::DRAW, 0, 0, 0)]),
            16,
            RootSigId::NULL
        )
        .is_ok()
    );
    // Too many constants for the root.
    let big = arg(IndirectArgKind::CONSTANT, 0, 0, 65);
    assert!(cmd(&sig(&[big, dispatch]), 300, RootSigId(1)).is_err());
}

#[test]
fn header_checks() {
    let good = synthetic_compute();
    assert!(Decoder::new(&good).is_ok());
    // Magic.
    let mut b = good.clone();
    b[0] ^= 0xFF;
    assert_eq!(first_error(&b), Error::BadMagic);
    // Proto version.
    let mut b = good.clone();
    b[8..12].copy_from_slice(&(PROTO_VERSION + 1).to_le_bytes());
    assert_eq!(
        first_error(&b),
        Error::BadProtoVersion {
            found: PROTO_VERSION + 1
        }
    );
    b[8..12].copy_from_slice(&0u32.to_le_bytes());
    assert_eq!(first_error(&b), Error::BadProtoVersion { found: 0 });
    // Header length and flags.
    let mut b = good.clone();
    b[12..14].copy_from_slice(&40u16.to_le_bytes());
    assert_eq!(first_error(&b), Error::BadHeader);
    let mut b = good.clone();
    b[14] = 1;
    assert_eq!(first_error(&b), Error::BadHeader);
    // Total length vs the block.
    let mut b = good.clone();
    b.extend_from_slice(&[0; 8]);
    assert!(matches!(first_error(&b), Error::BadTotalLength { .. }));
    let mut b = good.clone();
    b[24..28].copy_from_slice(&0u32.to_le_bytes());
    assert!(matches!(
        first_error(&b),
        Error::BadTotalLength { declared: 0, .. }
    ));
    // Too short to hold a header.
    for n in 0..STREAM_HEADER_SIZE {
        assert!(Decoder::new(&good[..n]).is_err(), "{n}");
    }
    assert!(Decoder::new(&[]).is_err());
}

#[test]
fn command_count_must_match() {
    let body = raw_cmd(0x0200, 1, 0, 16, &dispatch_payload());
    // Header counts two, block holds one.
    let block = block_with(2, &body);
    assert_eq!(
        first_error(&block),
        Error::CommandCountMismatch {
            declared: 2,
            found: 1
        }
    );
    // A count too large for the bytes is caught before anything is read.
    let block = block_with(1_000_000, &body);
    assert!(matches!(
        first_error(&block),
        Error::CommandCountMismatch {
            declared: 1_000_000,
            ..
        }
    ));
    // Header counts none, block holds one.
    let block = block_with(0, &body);
    assert_eq!(first_error(&block), Error::TrailingBytes);
    // Header counts one, block holds two.
    let mut two = body.clone();
    two.extend_from_slice(&body);
    let block = block_with(1, &two);
    assert_eq!(first_error(&block), Error::TrailingBytes);
}

#[test]
fn every_truncation_of_a_stream_is_refused() {
    for block in [synthetic_compute(), encode(&one_of_each())] {
        for n in 0..block.len() {
            // A prefix has a wrong total_len, so the header check fails ...
            assert!(Decoder::new(&block[..n]).is_err(), "prefix {n}");
        }
        // ... and a prefix whose header was patched to fit is cut mid-command.
        for n in (STREAM_HEADER_SIZE..block.len()).step_by(8) {
            let mut b = block[..n].to_vec();
            b[24..28].copy_from_slice(&(n as u32).to_le_bytes());
            let r = Decoder::new(&b).and_then(|d| d.collect_all());
            assert!(r.is_err(), "patched prefix {n}");
        }
    }
}

#[test]
fn the_decoder_stops_after_its_first_error() {
    let mut body = raw_cmd(0x0200, 1, 0, 16, &dispatch_payload());
    body.extend_from_slice(&raw_cmd(0x0A00, 1, 0, 0, &[]));
    body.extend_from_slice(&raw_cmd(0x0200, 1, 0, 16, &dispatch_payload()));
    let block = block_with(3, &body);
    let mut d = Decoder::new(&block).unwrap();
    assert!(d.next().unwrap().is_ok());
    assert_eq!(d.next().unwrap(), Err(Error::UnknownOpcode(0x0A00)));
    assert!(d.next().is_none());
    assert!(d.next().is_none());
}

#[test]
fn files() {
    let file = synthetic_dump();
    assert!(FileReader::looks_like_file(&file));
    assert!(!FileReader::looks_like_file(&synthetic_compute()));
    let blocks: Vec<_> = FileReader::new(&file)
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(blocks.len(), 2);
    assert_eq!(blocks[1], &synthetic_compute()[..]);
    // Header damage.
    let mut b = file.clone();
    b[0] = b'X';
    assert_eq!(FileReader::new(&b).unwrap_err(), Error::BadMagic);
    let mut b = file.clone();
    b[8..12].copy_from_slice(&2u32.to_le_bytes());
    assert!(matches!(
        FileReader::new(&b),
        Err(Error::BadProtoVersion { found: 2 })
    ));
    let mut b = file.clone();
    b[12..16].copy_from_slice(&(PROTO_VERSION + 1).to_le_bytes());
    assert!(matches!(
        FileReader::new(&b),
        Err(Error::BadProtoVersion { .. })
    ));
    let mut b = file.clone();
    b[16..18].copy_from_slice(&32u16.to_le_bytes());
    assert_eq!(FileReader::new(&b).unwrap_err(), Error::BadHeader);
    // Truncation anywhere: either the reader or one of the blocks complains.
    for n in 0..file.len() {
        let r = FileReader::new(&file[..n]).and_then(|r| r.collect::<Result<Vec<_>, _>>());
        assert!(r.is_err(), "prefix {n} of {FILE_HEADER_SIZE}+");
    }
    // A block count that lies.
    let mut b = file.clone();
    b[20..24].copy_from_slice(&3u32.to_le_bytes());
    let r: Result<Vec<_>, _> = FileReader::new(&b).unwrap().collect();
    assert_eq!(
        r.unwrap_err(),
        Error::CommandCountMismatch {
            declared: 3,
            found: 2
        }
    );
    // A count of 0 means "until the end".
    let mut b = file.clone();
    b[20..24].copy_from_slice(&0u32.to_le_bytes());
    assert_eq!(FileReader::new(&b).unwrap().count(), 2);
    // A garbage block after the good ones.
    let mut b = file.clone();
    b.extend_from_slice(&[0x55; 64]);
    assert!(
        FileReader::new(&b)
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .is_err()
    );
    // The writer refuses a bad block.
    let mut w = FileWriter::new("x");
    assert!(w.push_block(&[0; 40]).is_err());
    assert!(w.push_block(&synthetic_compute()[..40]).is_err());
}

#[test]
fn file_header_carries_caps_and_creator() {
    let file = synthetic_dump();
    let r = FileReader::new(&file).unwrap();
    assert_eq!(r.header().block_count, 2);
    assert_eq!(r.header().caps, Caps::METAL4 | Caps::UNIFIED_MEMORY);
    assert_eq!(&r.header().creator[..11], b"proto tests");
}
