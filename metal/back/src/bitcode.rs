//! LLVM bitstream: a module's bitcode read into a tree of blocks, records
//! and abbreviation definitions, and written back (the container of DXIL
//! holds LLVM 3.7 bitcode; the format is LLVM's "Bitstream" of
//! `BitCodes.h`: abbreviation ids 0 `END_BLOCK`, 1 `ENTER_SUBBLOCK`,
//! 2 `DEFINE_ABBREV`, 3 `UNABBREV_RECORD`, 4 and up the defined ones).
//!
//! What is read is what is written: each record keeps the abbreviation it
//! was written with, and block lengths are worked out again, so an edit that
//! changes the width of a value moves everything after it and nothing else.
//! The tree does not interpret the records; [`crate::mesh_fit`] does.

use std::collections::HashMap;
use std::fmt;
use std::rc::Rc;

/// What went wrong reading a bitstream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error(pub String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "bitcode: {}", self.0)
    }
}

impl std::error::Error for Error {}

type Result<T> = std::result::Result<T, Error>;

fn err<T>(m: impl Into<String>) -> Result<T> {
    Err(Error(m.into()))
}

/// One operand of an abbreviation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Literal(u64),
    Fixed(u32),
    Vbr(u32),
    /// Length, then elements of the operand that follows.
    Array,
    Char6,
    Blob,
}

pub type Abbrev = Rc<Vec<Op>>;

/// A record: `abbrev` is the abbreviation id it was written with (3 for an
/// unabbreviated one), `ops` the values after the code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    pub abbrev: u32,
    pub code: u64,
    pub ops: Vec<u64>,
    pub blob: Option<Vec<u8>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Item {
    Record(Record),
    Block(Block),
    Define(Abbrev),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    pub id: u32,
    /// The abbreviation id width inside.
    pub width: u32,
    pub items: Vec<Item>,
}

/// A whole stream: the magic and the top-level blocks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bitcode {
    pub magic: [u8; 4],
    pub blocks: Vec<Block>,
}

/// Where a block starts in a written stream: its id, how deep it is (a
/// top-level block is 1), and its first bit counted in words from the
/// magic, when that is a word boundary (a block after another one is).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placed {
    pub id: u32,
    pub depth: u32,
    pub word: Option<usize>,
}

const CHAR6: &[u8; 64] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789._";

/// Abbreviations that blocks of an id start with (`BLOCKINFO`).
type Info = HashMap<u32, Vec<Abbrev>>;

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl Reader<'_> {
    fn bits_left(&self) -> usize {
        self.data.len() * 8 - self.pos
    }

    fn read(&mut self, n: u32) -> Result<u64> {
        if n as usize > self.bits_left() {
            return err("the stream ends inside a value");
        }
        let mut v = 0u64;
        let mut got = 0;
        while got < n {
            let byte = self.data[self.pos / 8];
            let shift = (self.pos % 8) as u32;
            let take = (8 - shift).min(n - got);
            let bits = (u64::from(byte) >> shift) & ((1u64 << take) - 1);
            v |= bits << got;
            got += take;
            self.pos += take as usize;
        }
        Ok(v)
    }

    fn vbr(&mut self, width: u32) -> Result<u64> {
        if !(2..=32).contains(&width) {
            return err(format!("a VBR of width {width}"));
        }
        let hi = 1u64 << (width - 1);
        let mut v = 0u64;
        let mut shift = 0;
        loop {
            let chunk = self.read(width)?;
            if shift < 64 {
                v |= (chunk & (hi - 1)) << shift;
            }
            if chunk & hi == 0 {
                return Ok(v);
            }
            shift += width - 1;
            if shift > 70 {
                return err("a VBR longer than 64 bits");
            }
        }
    }

    fn align32(&mut self) -> Result<()> {
        let to = self.pos.next_multiple_of(32);
        if to > self.data.len() * 8 {
            return err("the stream ends before its alignment");
        }
        self.pos = to;
        Ok(())
    }

    fn scalar(&mut self, op: Op) -> Result<u64> {
        match op {
            Op::Literal(v) => Ok(v),
            Op::Fixed(w) => self.read(w),
            Op::Vbr(w) => self.vbr(w),
            Op::Char6 => Ok(u64::from(CHAR6[self.read(6)? as usize])),
            Op::Array | Op::Blob => err("an array or blob where a scalar is"),
        }
    }

    fn define(&mut self) -> Result<Abbrev> {
        let n = self.vbr(5)?;
        let mut ops = Vec::new();
        for _ in 0..n {
            if self.read(1)? == 1 {
                ops.push(Op::Literal(self.vbr(8)?));
            } else {
                let enc = self.read(3)?;
                ops.push(match enc {
                    1 => Op::Fixed(self.vbr(5)? as u32),
                    2 => Op::Vbr(self.vbr(5)? as u32),
                    3 => Op::Array,
                    4 => Op::Char6,
                    5 => Op::Blob,
                    _ => return err(format!("an abbreviation operand encoding {enc}")),
                });
            }
        }
        Ok(Rc::new(ops))
    }

    fn abbreviated(&mut self, id: u32, abbrev: &[Op]) -> Result<Record> {
        let mut vals = Vec::new();
        let mut blob = None;
        let mut i = 0;
        while i < abbrev.len() {
            match abbrev[i] {
                Op::Array => {
                    let n = self.vbr(6)?;
                    let elem = *abbrev
                        .get(i + 1)
                        .ok_or_else(|| Error("an array without an element type".into()))?;
                    for _ in 0..n {
                        vals.push(self.scalar(elem)?);
                    }
                    i += 1;
                }
                Op::Blob => {
                    let n = self.vbr(6)? as usize;
                    self.align32()?;
                    let at = self.pos / 8;
                    let bytes = self
                        .data
                        .get(at..at + n)
                        .ok_or_else(|| Error("a blob past the end".into()))?;
                    blob = Some(bytes.to_vec());
                    self.pos += n * 8;
                    self.align32()?;
                }
                op => vals.push(self.scalar(op)?),
            }
            i += 1;
        }
        if vals.is_empty() {
            return err("an abbreviated record without a code");
        }
        Ok(Record {
            abbrev: id,
            code: vals[0],
            ops: vals[1..].to_vec(),
            blob,
        })
    }

    fn block_body(&mut self, id: u32, width: u32, info: &mut Info) -> Result<Vec<Item>> {
        let mut abbrevs: Vec<Abbrev> = info.get(&id).cloned().unwrap_or_default();
        let mut items = Vec::new();
        let mut current: Option<u32> = None;
        loop {
            match self.read(width)? {
                0 => {
                    self.align32()?;
                    return Ok(items);
                }
                1 => items.push(Item::Block(self.sub_block(info)?)),
                2 => {
                    let a = self.define()?;
                    if id == 0 {
                        let to =
                            current.ok_or_else(|| Error("an abbreviation before SETBID".into()))?;
                        info.entry(to).or_default().push(a.clone());
                    } else {
                        abbrevs.push(a.clone());
                    }
                    items.push(Item::Define(a));
                }
                3 => {
                    let code = self.vbr(6)?;
                    let n = self.vbr(6)?;
                    if n as usize > self.bits_left() {
                        return err("more operands than bits");
                    }
                    let ops = (0..n).map(|_| self.vbr(6)).collect::<Result<Vec<_>>>()?;
                    if id == 0 && code == 1 {
                        current = ops.first().map(|&b| b as u32);
                    }
                    items.push(Item::Record(Record {
                        abbrev: 3,
                        code,
                        ops,
                        blob: None,
                    }));
                }
                n => {
                    let a = abbrevs
                        .get(n as usize - 4)
                        .cloned()
                        .ok_or_else(|| Error(format!("an undefined abbreviation {n}")))?;
                    let r = self.abbreviated(n as u32, &a)?;
                    if id == 0 && r.code == 1 {
                        current = r.ops.first().map(|&b| b as u32);
                    }
                    items.push(Item::Record(r));
                }
            }
        }
    }

    fn sub_block(&mut self, info: &mut Info) -> Result<Block> {
        let id = self.vbr(8)? as u32;
        let width = self.vbr(4)? as u32;
        if !(2..=32).contains(&width) {
            return err(format!("an abbreviation width of {width}"));
        }
        self.align32()?;
        let words = self.read(32)? as usize;
        let end = self.pos + words * 32;
        let items = self.block_body(id, width, info)?;
        if self.pos != end {
            return err(format!(
                "block {id} is {} bits, its length says {}",
                self.pos - (end - words * 32),
                words * 32
            ));
        }
        Ok(Block { id, width, items })
    }
}

impl Bitcode {
    /// Reads a stream: the magic `BC 0xC0DE` and top-level blocks (width 2).
    pub fn parse(data: &[u8]) -> Result<Bitcode> {
        if data.len() < 8 || data[..2] != *b"BC" || data[2..4] != [0xC0, 0xDE] {
            return err("not LLVM bitcode (no `BC` 0xC0DE magic)");
        }
        if !data.len().is_multiple_of(4) {
            return err("a stream that is not whole words");
        }
        let mut r = Reader { data, pos: 32 };
        let mut info = Info::new();
        let mut blocks = Vec::new();
        while r.bits_left() > 0 {
            if r.read(2)? != 1 {
                return err("something other than a block at the top level");
            }
            blocks.push(r.sub_block(&mut info)?);
        }
        Ok(Bitcode {
            magic: [data[0], data[1], data[2], data[3]],
            blocks,
        })
    }

    /// The stream.
    pub fn write(&self) -> Result<Vec<u8>> {
        Ok(self.write_placed()?.0)
    }

    /// The stream and where each block starts (see [`Placed`]).
    pub fn write_placed(&self) -> Result<(Vec<u8>, Vec<Placed>)> {
        let mut w = Writer::default();
        w.bytes.extend_from_slice(&self.magic);
        let mut info = Info::new();
        for b in &self.blocks {
            w.block(b, 2, 1, &mut info)?;
        }
        Ok((w.bytes, w.placed))
    }
}

#[derive(Default)]
struct Writer {
    bytes: Vec<u8>,
    acc: u64,
    nbits: u32,
    placed: Vec<Placed>,
}

impl Writer {
    fn write(&mut self, v: u64, n: u32) {
        debug_assert!(n == 64 || v >> n == 0, "{v:#x} in {n} bits");
        let mut v = v;
        let mut n = n;
        while n > 0 {
            let take = n.min(32);
            let part = v & ((1u64 << take) - 1);
            self.acc |= part << self.nbits;
            self.nbits += take;
            while self.nbits >= 8 {
                self.bytes.push(self.acc as u8);
                self.acc >>= 8;
                self.nbits -= 8;
            }
            v = if take == 64 { 0 } else { v >> take };
            n -= take;
        }
    }

    fn vbr(&mut self, mut v: u64, width: u32) {
        let hi = 1u64 << (width - 1);
        while v >= hi {
            self.write((v & (hi - 1)) | hi, width);
            v >>= width - 1;
        }
        self.write(v, width);
    }

    fn align32(&mut self) {
        if self.nbits > 0 {
            self.write(0, 8 - self.nbits);
        }
        while !self.bytes.len().is_multiple_of(4) {
            self.bytes.push(0);
        }
    }

    fn scalar(&mut self, op: Op, v: u64) -> Result<()> {
        match op {
            Op::Literal(l) if l == v => {}
            Op::Literal(l) => return err(format!("{v} where the abbreviation says {l}")),
            Op::Fixed(w) => {
                if w < 64 && v >> w != 0 {
                    return err(format!("{v} does not fit {w} bits"));
                }
                self.write(v, w);
            }
            Op::Vbr(w) => self.vbr(v, w),
            Op::Char6 => match CHAR6.iter().position(|&c| u64::from(c) == v) {
                Some(i) => self.write(i as u64, 6),
                None => return err(format!("{v} is not a char6")),
            },
            Op::Array | Op::Blob => return err("an array or blob where a scalar is"),
        }
        Ok(())
    }

    fn define(&mut self, a: &[Op], width: u32) {
        self.write(2, width);
        self.vbr(a.len() as u64, 5);
        for op in a {
            match *op {
                Op::Literal(v) => {
                    self.write(1, 1);
                    self.vbr(v, 8);
                }
                Op::Fixed(w) => {
                    self.write(0, 1);
                    self.write(1, 3);
                    self.vbr(u64::from(w), 5);
                }
                Op::Vbr(w) => {
                    self.write(0, 1);
                    self.write(2, 3);
                    self.vbr(u64::from(w), 5);
                }
                Op::Array => {
                    self.write(0, 1);
                    self.write(3, 3);
                }
                Op::Char6 => {
                    self.write(0, 1);
                    self.write(4, 3);
                }
                Op::Blob => {
                    self.write(0, 1);
                    self.write(5, 3);
                }
            }
        }
    }

    fn record(&mut self, r: &Record, width: u32, abbrevs: &[Abbrev]) -> Result<()> {
        self.write(u64::from(r.abbrev), width);
        if r.abbrev == 3 {
            self.vbr(r.code, 6);
            self.vbr(r.ops.len() as u64, 6);
            for &o in &r.ops {
                self.vbr(o, 6);
            }
            return Ok(());
        }
        let a = abbrevs
            .get(r.abbrev as usize - 4)
            .ok_or_else(|| Error(format!("an undefined abbreviation {}", r.abbrev)))?
            .clone();
        let vals: Vec<u64> = std::iter::once(r.code)
            .chain(r.ops.iter().copied())
            .collect();
        let mut next = 0;
        let mut i = 0;
        while i < a.len() {
            match a[i] {
                Op::Array => {
                    let n = vals.len() - next;
                    self.vbr(n as u64, 6);
                    let elem = a[i + 1];
                    for &v in &vals[next..] {
                        self.scalar(elem, v)?;
                    }
                    next = vals.len();
                    i += 1;
                }
                Op::Blob => {
                    let blob = r.blob.as_deref().unwrap_or(&[]);
                    self.vbr(blob.len() as u64, 6);
                    self.align32();
                    self.bytes.extend_from_slice(blob);
                    self.align32();
                }
                op => {
                    let v = *vals
                        .get(next)
                        .ok_or_else(|| Error("a record shorter than its abbreviation".into()))?;
                    next += 1;
                    self.scalar(op, v)?;
                }
            }
            i += 1;
        }
        if next != vals.len() {
            return err("a record longer than its abbreviation");
        }
        Ok(())
    }

    fn block(&mut self, b: &Block, outer: u32, depth: u32, info: &mut Info) -> Result<()> {
        let bit = self.bytes.len() * 8 + self.nbits as usize;
        self.placed.push(Placed {
            id: b.id,
            depth,
            word: bit.is_multiple_of(32).then_some(bit / 32),
        });
        self.write(1, outer);
        self.vbr(u64::from(b.id), 8);
        self.vbr(u64::from(b.width), 4);
        self.align32();
        let len_at = self.bytes.len();
        self.bytes.extend_from_slice(&[0; 4]);
        let mut abbrevs: Vec<Abbrev> = info.get(&b.id).cloned().unwrap_or_default();
        let mut current: Option<u32> = None;
        for item in &b.items {
            match item {
                Item::Record(r) => {
                    self.record(r, b.width, &abbrevs)?;
                    if b.id == 0 && r.code == 1 {
                        current = r.ops.first().map(|&x| x as u32);
                    }
                }
                Item::Define(a) => {
                    self.define(a, b.width);
                    if b.id == 0 {
                        let to =
                            current.ok_or_else(|| Error("an abbreviation before SETBID".into()))?;
                        info.entry(to).or_default().push(a.clone());
                    } else {
                        abbrevs.push(a.clone());
                    }
                }
                Item::Block(inner) => self.block(inner, b.width, depth + 1, info)?,
            }
        }
        self.write(0, b.width);
        self.align32();
        let words = (self.bytes.len() - len_at - 4) / 4;
        self.bytes[len_at..len_at + 4].copy_from_slice(&(words as u32).to_le_bytes());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(abbrev: u32, code: u64, ops: &[u64]) -> Item {
        Item::Record(Record {
            abbrev,
            code,
            ops: ops.to_vec(),
            blob: None,
        })
    }

    /// A stream with every kind of operand: BLOCKINFO with an abbreviation
    /// for block 9, a block using it, an array of char6, a blob, an
    /// unabbreviated record with large values, and a nested block.
    fn sample() -> Bitcode {
        let info = Block {
            id: 0,
            width: 2,
            items: vec![
                rec(3, 1, &[9]),
                Item::Define(Rc::new(vec![Op::Literal(5), Op::Fixed(7), Op::Vbr(6)])),
            ],
        };
        let inner = Block {
            id: 10,
            width: 3,
            items: vec![rec(3, 2, &[u64::MAX, 0, 1 << 40])],
        };
        let body = Block {
            id: 9,
            width: 4,
            items: vec![
                rec(4, 5, &[100, 70_000]),
                Item::Define(Rc::new(vec![Op::Literal(7), Op::Array, Op::Char6])),
                rec(5, 7, &[b'a'.into(), b'Z'.into(), b'_'.into()]),
                Item::Define(Rc::new(vec![Op::Literal(8), Op::Vbr(6), Op::Blob])),
                Item::Record(Record {
                    abbrev: 6,
                    code: 8,
                    ops: vec![3],
                    blob: Some(vec![1, 2, 3, 4, 5]),
                }),
                Item::Block(inner),
            ],
        };
        Bitcode {
            magic: [b'B', b'C', 0xC0, 0xDE],
            blocks: vec![info, body],
        }
    }

    #[test]
    fn what_is_written_reads_back_and_writes_the_same_bytes() {
        let bc = sample();
        let bytes = bc.write().unwrap();
        assert!(bytes.len().is_multiple_of(4));
        let back = Bitcode::parse(&bytes).unwrap();
        assert_eq!(back, bc);
        assert_eq!(back.write().unwrap(), bytes);
    }

    #[test]
    fn a_wider_value_moves_what_follows_and_the_lengths_follow() {
        let mut bc = sample();
        let body = &mut bc.blocks[1];
        let Item::Record(r) = &mut body.items[0] else {
            unreachable!()
        };
        r.ops[1] = 1 << 40;
        let bytes = bc.write().unwrap();
        assert_eq!(Bitcode::parse(&bytes).unwrap(), bc);
    }

    #[test]
    fn a_value_that_does_not_fit_its_abbreviation_is_an_error_not_a_wrong_stream() {
        let mut bc = sample();
        let body = &mut bc.blocks[1];
        let Item::Record(r) = &mut body.items[0] else {
            unreachable!()
        };
        r.ops[0] = 200; // Fixed(7) holds up to 127
        assert!(bc.write().is_err());
    }

    #[test]
    fn broken_streams_are_refused() {
        assert!(Bitcode::parse(b"nope").is_err());
        assert!(Bitcode::parse(b"BC\xC0\xDE\x01\x00").is_err());
        let mut bytes = sample().write().unwrap();
        bytes.truncate(bytes.len() - 8);
        assert!(Bitcode::parse(&bytes).is_err());
    }
}
