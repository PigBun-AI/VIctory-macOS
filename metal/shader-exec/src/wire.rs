//! The pipe protocol (version 1), a plain length-prefixed little-endian
//! byte format. `crates/victory-shader` (`emit/host_exec/wire.rs`, branch
//! `mac/shader-harness`) writes the other half; the golden vectors in the
//! tests of both sides pin the bytes. Rules: a frame is `u32 length` then the
//! payload; a request payload is `u8 op` then its body; a reply payload is
//! `u8 status` then its body (a text for every status but `OK`).

use std::io::{Read, Write};

pub const VERSION: u32 = 1;

pub mod op {
    pub const HELLO: u8 = 1;
    pub const COMPILE: u8 = 2;
    pub const DISPATCH: u8 = 3;
}

pub mod status {
    /// Done; the body is the answer.
    pub const OK: u8 = 0;
    /// This device or converter does not take the program: the test skips it
    /// (MSC refused a construct, a limit, a format the device lacks).
    pub const UNSUPPORTED: u8 = 1;
    /// Something that should have worked did not (a wrong argument, a Metal
    /// error, a timeout): the test fails.
    pub const FAILED: u8 = 2;
}

/// Hard limit of one frame (a texture or two is far below it).
pub const MAX_FRAME: usize = 1 << 28;

pub fn read_frame(r: &mut impl Read) -> std::io::Result<Option<Vec<u8>>> {
    let mut len = [0u8; 4];
    match r.read_exact(&mut len) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let len = u32::from_le_bytes(len) as usize;
    if len > MAX_FRAME {
        return Err(std::io::Error::other(format!("frame of {len} bytes")));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)?;
    Ok(Some(buf))
}

pub fn write_frame(w: &mut impl Write, payload: &[u8]) -> std::io::Result<()> {
    w.write_all(&(payload.len() as u32).to_le_bytes())?;
    w.write_all(payload)?;
    w.flush()
}

/// A payload being written.
#[derive(Default)]
pub struct W(pub Vec<u8>);

impl W {
    pub fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    pub fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    pub fn f32(&mut self, v: f32) {
        self.u32(v.to_bits());
    }
    pub fn bytes(&mut self, v: &[u8]) {
        self.u32(v.len() as u32);
        self.0.extend_from_slice(v);
    }
    pub fn str(&mut self, v: &str) {
        self.bytes(v.as_bytes());
    }
    pub fn u32s(&mut self, v: &[u32]) {
        self.u32(v.len() as u32);
        for x in v {
            self.u32(*x);
        }
    }
}

/// A payload being read; every getter fails on a short payload.
pub struct R<'a> {
    b: &'a [u8],
    at: usize,
}

type Res<T> = Result<T, String>;

impl<'a> R<'a> {
    pub fn new(b: &'a [u8]) -> Self {
        R { b, at: 0 }
    }
    fn take(&mut self, n: usize) -> Res<&'a [u8]> {
        let end = self
            .at
            .checked_add(n)
            .filter(|&e| e <= self.b.len())
            .ok_or_else(|| format!("short payload: {n} bytes wanted at {}", self.at))?;
        let s = &self.b[self.at..end];
        self.at = end;
        Ok(s)
    }
    pub fn u8(&mut self) -> Res<u8> {
        Ok(self.take(1)?[0])
    }
    pub fn u32(&mut self) -> Res<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().expect("4 bytes")))
    }
    pub fn f32(&mut self) -> Res<f32> {
        Ok(f32::from_bits(self.u32()?))
    }
    pub fn bytes(&mut self) -> Res<&'a [u8]> {
        let n = self.u32()? as usize;
        self.take(n)
    }
    pub fn str(&mut self) -> Res<String> {
        Ok(String::from_utf8_lossy(self.bytes()?).into_owned())
    }
    pub fn u32s(&mut self) -> Res<Vec<u32>> {
        let n = self.u32()? as usize;
        let raw = self.take(n.checked_mul(4).ok_or("overflow")?)?;
        Ok(raw
            .chunks_exact(4)
            .map(|c| u32::from_le_bytes(c.try_into().expect("4 bytes")))
            .collect())
    }
    /// The bytes read so far (the cache key of a request is its prefix).
    pub fn consumed(&self) -> &'a [u8] {
        &self.b[..self.at]
    }
    pub fn end(&self) -> Res<()> {
        if self.at == self.b.len() {
            Ok(())
        } else {
            Err(format!("{} trailing bytes", self.b.len() - self.at))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scalars_round_trip_and_short_payloads_fail() {
        let mut w = W::default();
        w.u8(7);
        w.u32(0x0102_0304);
        w.f32(1.5);
        w.bytes(b"ab");
        w.u32s(&[1, 2]);
        assert_eq!(
            w.0,
            [
                7, 4, 3, 2, 1, 0, 0, 0xC0, 0x3F, 2, 0, 0, 0, b'a', b'b', 2, 0, 0, 0, 1, 0, 0, 0,
                2, 0, 0, 0
            ]
        );
        let mut r = R::new(&w.0);
        assert_eq!(r.u8().unwrap(), 7);
        assert_eq!(r.u32().unwrap(), 0x0102_0304);
        assert_eq!(r.f32().unwrap(), 1.5);
        assert_eq!(r.bytes().unwrap(), b"ab");
        assert_eq!(r.u32s().unwrap(), vec![1, 2]);
        r.end().unwrap();
        assert!(R::new(&w.0[..3]).u32().is_ok() == false);
        assert!(R::new(&[5, 0, 0, 0, 1]).bytes().is_err());
    }

    #[test]
    fn frames_round_trip() {
        let mut v = Vec::new();
        write_frame(&mut v, b"hello").unwrap();
        let mut c = std::io::Cursor::new(v);
        assert_eq!(read_frame(&mut c).unwrap().unwrap(), b"hello");
        assert!(read_frame(&mut c).unwrap().is_none());
    }
}
