//! The top-level argument buffer (TLAB) layout of a root signature.
//!
//! A shader converted by Metal Shader Converter (MSC) reads its root
//! parameters from one buffer bound at bind point 2: root constants inline,
//! root descriptors as 64-bit GPU addresses, descriptor tables as the 64-bit
//! GPU address of their first entry in the descriptor heap. Where each
//! parameter sits is MSC's decision. **MSC is not installed here, so there
//! is no reflection yet** (`IRRootSignatureGetResourceLocations` and the
//! reflection JSON would give the offsets). Until then this file lays the
//! TLAB out by the rules of Apple's binding-model guide:
//!
//! | parameter | bytes | alignment |
//! |---|---|---|
//! | root constants (`num_32bit_values` values) | 4 per value | 4 |
//! | root descriptor (CBV, SRV, UAV): GPU address | 8 | 8 |
//! | descriptor table: GPU address of its first heap entry | 8 | 8 |
//!
//! in root-parameter order, each parameter at the next offset that meets its
//! alignment (so an odd number of constants before a pointer leaves a 4-byte
//! pad), the total rounded up to 8. Static samplers take no room.
//!
//! **Source**: `apple/game-porting-toolkit`,
//! `docs/metal-shader-converter/binding-model.md` ("Explicit layout via root
//! signatures": "...calculate the offsets of each resource ... while
//! observing that Metal Shader Converter adds padding to achieve the correct
//! alignment for each resource type: pointers 8-byte alignment, 32-bit
//! constants 4-byte alignment") and `reference.md` (a root signature with a
//! table, a CBV and 4 constants), commit `4f42344`, 2026-06-07.
//!
//! **待用 MSC 反射复核** (to be checked against MSC reflection). What the
//! guide does not settle, and a reflection result may change:
//!
//! * the order: it says to take "the size of the root constants in bytes (if
//!   present), and adding the resource index times 8", which can be read as
//!   constants first. Here it is root-parameter order, as MSC's root
//!   signature model (the same as the original API's) suggests;
//! * static samplers: the guide's example has one static sampler and one
//!   table and says the converted shader has two 8-byte entries, the second
//!   "a table with one sampler". Here static samplers take no room;
//! * the TLAB's own alignment (8 here, 16 when allocated).
//!
//! [`TlabLayout::from_reflection`] takes the offsets reflection reports and
//! replaces the rule-based layout of a root signature
//! (`Executor::set_tlab_layout`), so nothing else has to change.

use victory_metal_proto::root::RootSigDesc;
use victory_metal_proto::types::RootParamKind;

/// Most root constants one parameter can have (`proto::cmd::MAX_ROOT_CONSTANTS`).
const MAX_CONSTANTS_PER_PARAM: u32 = 64;
/// Largest TLAB this crate builds, bytes.
pub const MAX_TLAB_BYTES: u32 = 4096;

/// What one slot of the TLAB holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlotKind {
    /// `count` 32-bit constants.
    Constants {
        /// Number of 32-bit values.
        count: u32,
    },
    /// A GPU address (`RootParamKind::CBV`, `SRV` or `UAV`).
    Descriptor(RootParamKind),
    /// The GPU address of a heap slot; `sampler` says it points into the
    /// sampler heap.
    Table {
        /// The table's ranges are samplers.
        sampler: bool,
    },
}

/// One root parameter's place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Slot {
    /// What it holds.
    pub kind: SlotKind,
    /// Byte offset in the TLAB.
    pub offset: u32,
    /// Bytes it takes.
    pub size: u32,
}

/// Why a layout could not be made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LayoutError {
    /// The description is not valid (what).
    Invalid(String),
}

/// Where each root parameter lives in the TLAB.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TlabLayout {
    slots: Vec<Slot>,
    size: u32,
    from_reflection: bool,
}

fn align_up(v: u32, a: u32) -> u32 {
    (v + a - 1) & !(a - 1)
}

fn slot_shape(desc: &RootSigDesc, index: usize) -> Result<(SlotKind, u32, u32), LayoutError> {
    let p = &desc.params[index];
    let bad = |what: &str| LayoutError::Invalid(format!("root parameter {index}: {what}"));
    match p.kind {
        RootParamKind::CONSTANTS => {
            let n = p.num_32bit_values;
            if n == 0 || n > MAX_CONSTANTS_PER_PARAM {
                return Err(bad("constants must be 1..=64 values"));
            }
            Ok((SlotKind::Constants { count: n }, 4 * n, 4))
        }
        RootParamKind::CBV | RootParamKind::SRV | RootParamKind::UAV => {
            Ok((SlotKind::Descriptor(p.kind), 8, 8))
        }
        RootParamKind::TABLE => {
            let start = p.range_start as usize;
            let end = start + p.range_count as usize;
            let ranges = desc
                .ranges
                .get(start..end)
                .ok_or_else(|| bad("table runs past the range array"))?;
            let samplers = ranges
                .iter()
                .filter(|r| r.kind == victory_metal_proto::types::RangeKind::SAMPLER)
                .count();
            if samplers != 0 && samplers != ranges.len() {
                return Err(bad("a table cannot mix sampler and resource ranges"));
            }
            Ok((
                SlotKind::Table {
                    sampler: samplers != 0,
                },
                8,
                8,
            ))
        }
        _ => Err(bad("unknown kind")),
    }
}

impl TlabLayout {
    /// The layout by the binding-model rules in the module docs.
    pub fn from_desc(desc: &RootSigDesc) -> Result<TlabLayout, LayoutError> {
        let mut slots = Vec::with_capacity(desc.params.len());
        let mut at = 0u32;
        for i in 0..desc.params.len() {
            let (kind, size, align) = slot_shape(desc, i)?;
            let offset = align_up(at, align);
            at = offset + size;
            if at > MAX_TLAB_BYTES {
                return Err(LayoutError::Invalid(format!(
                    "the argument buffer would exceed {MAX_TLAB_BYTES} bytes"
                )));
            }
            slots.push(Slot { kind, offset, size });
        }
        Ok(TlabLayout {
            slots,
            size: align_up(at, 8),
            from_reflection: false,
        })
    }

    /// The layout MSC reflection reports: `offsets[i]` is the byte offset of
    /// root parameter `i`, `size` the whole buffer. Checked against the
    /// description: one offset per parameter, aligned, inside `size`, no
    /// overlap.
    pub fn from_reflection(
        desc: &RootSigDesc,
        offsets: &[u32],
        size: u32,
    ) -> Result<TlabLayout, LayoutError> {
        if offsets.len() != desc.params.len() {
            return Err(LayoutError::Invalid(format!(
                "{} offsets for {} root parameters",
                offsets.len(),
                desc.params.len()
            )));
        }
        if size > MAX_TLAB_BYTES {
            return Err(LayoutError::Invalid(format!(
                "the argument buffer would exceed {MAX_TLAB_BYTES} bytes"
            )));
        }
        let mut slots = Vec::with_capacity(offsets.len());
        for (i, &offset) in offsets.iter().enumerate() {
            let (kind, bytes, align) = slot_shape(desc, i)?;
            if !offset.is_multiple_of(align)
                || u64::from(offset) + u64::from(bytes) > u64::from(size)
            {
                return Err(LayoutError::Invalid(format!(
                    "root parameter {i} at {offset} (+{bytes}) does not fit {size} bytes at alignment {align}"
                )));
            }
            slots.push(Slot {
                kind,
                offset,
                size: bytes,
            });
        }
        let mut by_offset: Vec<&Slot> = slots.iter().collect();
        by_offset.sort_by_key(|s| s.offset);
        if by_offset
            .windows(2)
            .any(|w| w[0].offset + w[0].size > w[1].offset)
        {
            return Err(LayoutError::Invalid("two root parameters overlap".into()));
        }
        Ok(TlabLayout {
            slots,
            size,
            from_reflection: true,
        })
    }

    /// Bytes of the whole argument buffer (0 when there are no parameters).
    pub fn size(&self) -> u32 {
        self.size
    }

    /// The slot of root parameter `index`.
    pub fn slot(&self, index: u32) -> Option<&Slot> {
        self.slots.get(index as usize)
    }

    /// Number of root parameters.
    pub fn len(&self) -> usize {
        self.slots.len()
    }

    /// No root parameters.
    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// Whether the offsets came from MSC reflection rather than the rules.
    pub fn from_reflection_data(&self) -> bool {
        self.from_reflection
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use victory_metal_proto::root::{DescriptorRange, RootParam, UNBOUNDED};
    use victory_metal_proto::types::{RangeKind, ShaderVisibility};

    fn constants(n: u32) -> RootParam {
        RootParam {
            kind: RootParamKind::CONSTANTS,
            num_32bit_values: n,
            ..RootParam::default()
        }
    }
    fn descriptor(kind: RootParamKind) -> RootParam {
        RootParam {
            kind,
            ..RootParam::default()
        }
    }
    fn table(start: u32, count: u32) -> RootParam {
        RootParam {
            kind: RootParamKind::TABLE,
            visibility: ShaderVisibility::ALL,
            range_start: start,
            range_count: count,
            ..RootParam::default()
        }
    }
    fn range(kind: RangeKind) -> DescriptorRange {
        DescriptorRange {
            kind,
            num_descriptors: UNBOUNDED,
            ..DescriptorRange::default()
        }
    }
    fn desc(params: Vec<RootParam>, ranges: Vec<DescriptorRange>) -> RootSigDesc {
        RootSigDesc {
            params,
            ranges,
            ..RootSigDesc::default()
        }
    }

    fn offsets(l: &TlabLayout) -> Vec<u32> {
        (0..l.len() as u32)
            .map(|i| l.slot(i).unwrap().offset)
            .collect()
    }

    #[test]
    fn parameters_follow_each_other_with_their_alignment() {
        // The guide's JSON example: table, CBV, 4 constants.
        let d = desc(
            vec![table(0, 1), descriptor(RootParamKind::CBV), constants(4)],
            vec![range(RangeKind::SRV)],
        );
        let l = TlabLayout::from_desc(&d).unwrap();
        assert_eq!(offsets(&l), [0, 8, 16]);
        assert_eq!(l.size(), 32);
        // The synthetic golden signature: 2 constants, a CBV, a table.
        let d = desc(
            vec![constants(2), descriptor(RootParamKind::CBV), table(0, 2)],
            vec![range(RangeKind::SRV), range(RangeKind::UAV)],
        );
        let l = TlabLayout::from_desc(&d).unwrap();
        assert_eq!(offsets(&l), [0, 8, 16]);
        assert_eq!(l.size(), 24);
    }

    #[test]
    fn an_odd_number_of_constants_pads_before_a_pointer() {
        let d = desc(
            vec![constants(3), descriptor(RootParamKind::UAV), constants(1)],
            vec![],
        );
        let l = TlabLayout::from_desc(&d).unwrap();
        // 12 bytes of constants, 4 bytes pad, the pointer at 16, one more
        // constant at 24; the total is rounded to 8.
        assert_eq!(offsets(&l), [0, 16, 24]);
        assert_eq!(l.size(), 32);
        // Constants after constants need no pad.
        let d = desc(vec![constants(1), constants(1), constants(1)], vec![]);
        let l = TlabLayout::from_desc(&d).unwrap();
        assert_eq!(offsets(&l), [0, 4, 8]);
        assert_eq!(l.size(), 16, "12 bytes rounded up to 8");
    }

    #[test]
    fn tables_know_whether_they_point_into_the_sampler_heap() {
        let d = desc(
            vec![table(0, 1), table(1, 2)],
            vec![
                range(RangeKind::SAMPLER),
                range(RangeKind::CBV),
                range(RangeKind::UAV),
            ],
        );
        let l = TlabLayout::from_desc(&d).unwrap();
        assert_eq!(l.slot(0).unwrap().kind, SlotKind::Table { sampler: true });
        assert_eq!(l.slot(1).unwrap().kind, SlotKind::Table { sampler: false });
        let mixed = desc(
            vec![table(0, 2)],
            vec![range(RangeKind::SAMPLER), range(RangeKind::SRV)],
        );
        assert!(TlabLayout::from_desc(&mixed).is_err());
    }

    #[test]
    fn no_parameters_no_buffer_and_bad_descriptions_are_errors() {
        let l = TlabLayout::from_desc(&desc(vec![], vec![])).unwrap();
        assert_eq!((l.size(), l.is_empty()), (0, true));
        assert!(TlabLayout::from_desc(&desc(vec![constants(0)], vec![])).is_err());
        assert!(TlabLayout::from_desc(&desc(vec![constants(65)], vec![])).is_err());
        assert!(TlabLayout::from_desc(&desc(vec![table(0, 1)], vec![])).is_err());
        let mut p = constants(1);
        p.kind = RootParamKind(99);
        assert!(TlabLayout::from_desc(&desc(vec![p], vec![])).is_err());
        // Too big: 64 parameters of 64 constants.
        let big = desc(vec![constants(64); 64], vec![]);
        assert!(TlabLayout::from_desc(&big).is_err());
    }

    #[test]
    fn reflection_offsets_replace_the_rules_when_they_make_sense() {
        let d = desc(
            vec![table(0, 1), descriptor(RootParamKind::CBV), constants(4)],
            vec![range(RangeKind::SRV)],
        );
        // Constants first, as the other reading of the guide would put them.
        let l = TlabLayout::from_reflection(&d, &[16, 24, 0], 32).unwrap();
        assert_eq!(offsets(&l), [16, 24, 0]);
        assert!(l.from_reflection_data());
        assert_eq!(l.size(), 32);
        // Misaligned, outside, overlapping, wrong count.
        assert!(TlabLayout::from_reflection(&d, &[4, 24, 0], 32).is_err());
        assert!(TlabLayout::from_reflection(&d, &[16, 24, 28], 32).is_err());
        assert!(TlabLayout::from_reflection(&d, &[16, 24, 20], 32).is_err());
        assert!(TlabLayout::from_reflection(&d, &[0, 8], 32).is_err());
        assert!(!TlabLayout::from_desc(&d).unwrap().from_reflection_data());
    }
}
