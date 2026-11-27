//! DXIL signature parts (`ISG1` / `OSG1`): the elements of a translated
//! shader's inputs and outputs, as the translator writes them (header: element
//! count and offset; 32 bytes an element: stream, name offset, index, system
//! value, component type, register, mask, read/write mask).

/// One signature element.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Element {
    pub name: String,
    pub index: u32,
    /// `D3D_NAME`: 0 for an ordinary parameter.
    pub system_value: u32,
    /// 1 uint, 2 sint, 3 float.
    pub comp: u32,
    pub register: u32,
    pub mask: u8,
}

impl Element {
    /// Components used.
    pub fn width(&self) -> u32 {
        self.mask.count_ones().max(1)
    }
}

fn word(p: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(p.get(at..at + 4)?.try_into().ok()?))
}

/// The elements of a signature part (empty when it is damaged).
pub fn parse(part: &[u8]) -> Vec<Element> {
    let (Some(count), Some(base)) = (word(part, 0), word(part, 4)) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for i in 0..count as usize {
        let at = base as usize + 32 * i;
        let (Some(name_at), Some(index), Some(sv), Some(comp), Some(register)) = (
            word(part, at + 4),
            word(part, at + 8),
            word(part, at + 12),
            word(part, at + 16),
            word(part, at + 20),
        ) else {
            return Vec::new();
        };
        let Some(mask) = part.get(at + 24).copied() else {
            return Vec::new();
        };
        let Some(tail) = part.get(name_at as usize..) else {
            return Vec::new();
        };
        let len = tail.iter().position(|&b| b == 0).unwrap_or(tail.len());
        out.push(Element {
            name: String::from_utf8_lossy(&tail[..len]).into_owned(),
            index,
            system_value: sv,
            comp,
            register,
            mask,
        });
    }
    out
}

/// The input and output signatures of a DXIL container.
pub fn of_container(dxil: &[u8]) -> (Vec<Element>, Vec<Element>) {
    use victory_shader::{Container, FourCC};
    let Ok(c) = Container::parse(dxil) else {
        return (Vec::new(), Vec::new());
    };
    let get = |cc| c.part(cc).map(|p| parse(p.data)).unwrap_or_default();
    (get(FourCC::ISG1), get(FourCC::OSG1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_signature_part_reads_back() {
        // One element TEXCOORD1, float, register 2, mask xyz.
        let mut p = Vec::new();
        p.extend(1u32.to_le_bytes());
        p.extend(8u32.to_le_bytes());
        for v in [0u32, 8 + 32, 1, 0, 3, 2] {
            p.extend(v.to_le_bytes());
        }
        p.extend([0b0111, 0, 0, 0]);
        p.extend([0, 0, 0, 0]);
        p.extend(b"TEXCOORD\0");
        let els = parse(&p);
        assert_eq!(els.len(), 1);
        assert_eq!(els[0].name, "TEXCOORD");
        assert_eq!((els[0].index, els[0].comp, els[0].register), (1, 3, 2));
        assert_eq!(els[0].width(), 3);
    }

    #[test]
    fn a_damaged_part_is_empty() {
        assert!(parse(&[1, 0, 0, 0]).is_empty());
        assert!(parse(&[]).is_empty());
    }
}
