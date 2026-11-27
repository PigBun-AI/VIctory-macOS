//! Rule 4: changing the protocol means raising `PROTO_VERSION`.
//!
//! `layout_fingerprint()` hashes every wire-visible fact. This table pins it
//! per version. If you changed a payload, a sync block, a header or the
//! opcode list, `fingerprint_matches_the_pinned_one_for_this_version` fails
//! until you raise `PROTO_VERSION`, add a new arm with the new fingerprint
//! (keep the old ones), regenerate the golden files and `docs/metal/proto.md`.

use victory_metal_proto::PROTO_VERSION;
use victory_metal_proto::registry::layout_fingerprint;

const PINNED: &[(u32, u64)] = &[
    (1, 0xa028_6369_07f7_87a9),
    (2, 0x8de9_d9a2_a9f0_459a),
    (3, 0xe307_f36d_584c_63ba),
    (4, 0xf261_54c0_585d_edcb),
];

#[test]
fn fingerprint_matches_the_pinned_one_for_this_version() {
    let pinned = PINNED
        .iter()
        .find(|(v, _)| *v == PROTO_VERSION)
        .map(|p| p.1);
    let Some(pinned) = pinned else {
        panic!(
            "PROTO_VERSION {PROTO_VERSION} has no pinned fingerprint: add ({PROTO_VERSION}, {:#018x})",
            layout_fingerprint()
        );
    };
    assert_eq!(
        layout_fingerprint(),
        pinned,
        "the wire format changed but PROTO_VERSION ({PROTO_VERSION}) did not: see proto/README.md, 'Changing the protocol'"
    );
}

#[test]
fn versions_are_pinned_in_order() {
    let mut last = 0;
    for (v, _) in PINNED {
        assert!(*v > last);
        last = *v;
    }
    assert_eq!(
        last, PROTO_VERSION,
        "the newest pinned version must be the current one"
    );
}
