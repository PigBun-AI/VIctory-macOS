//! `VICTORY_METAL_DUMP_PRESENT=<present numbers>@<directory>` (diagnostics, off by default).
//!
//! For each listed present (the n-th present of a swap chain, counted from 1; for the title's single
//! swap chain that is the title's present number) the back writes two PNGs into the directory, after the
//! present kernel has run and before the drawable goes to the display:
//!
//! * `present-<n>-source.png`: the source back buffer, read through a compute kernel into a readback
//!   buffer (the texture is private), as 8-bit RGBA (the texture's logical channel order, `x * 255`
//!   rounded; exact for 8-bit unorm back buffers, lossy for 10-bit and half-float ones);
//! * `present-<n>-drawable.png`: the whole drawable the display gets (BGRA bytes swapped to RGBA).
//!
//! With the source and the drawable of equal size the two files are byte-equal in the pixels; with
//! different sizes the drawable is the nearest-neighbour scaling of the source (`present_copy`).
//!
//! The PNGs are written with stored (uncompressed) deflate blocks: no dependency, big, but trivially
//! correct. [`read_png_rgba8`] reads exactly what [`write_png_rgba8`] writes.

use std::path::{Path, PathBuf};

/// What `VICTORY_METAL_DUMP_PRESENT` asked for.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DumpSpec {
    /// Present numbers (from 1).
    pub presents: Vec<u64>,
    /// Where the files go.
    pub dir: PathBuf,
}

impl DumpSpec {
    /// Parses `300,1322@/some/dir`. `None` for an empty or malformed value.
    pub fn parse(s: &str) -> Option<DumpSpec> {
        let (list, dir) = s.split_once('@')?;
        let presents: Vec<u64> = list
            .split(',')
            .filter_map(|n| n.trim().parse().ok())
            .collect();
        (!presents.is_empty() && !dir.is_empty()).then(|| DumpSpec {
            presents,
            dir: PathBuf::from(dir),
        })
    }

    /// From the environment.
    pub fn from_env() -> Option<DumpSpec> {
        DumpSpec::parse(&std::env::var("VICTORY_METAL_DUMP_PRESENT").ok()?)
    }

    pub fn wants(&self, present: u64) -> bool {
        self.presents.contains(&present)
    }

    pub fn path(&self, present: u64, what: &str) -> PathBuf {
        self.dir.join(format!("present-{present}-{what}.png"))
    }
}

/// The kernel that reads the source texture into a buffer of RGBA8 texels (slots as `present_copy`).
pub const DUMP_KERNEL: &str = "#include <metal_stdlib>\nusing namespace metal;\n\
struct SrcEntry { ulong va; texture2d<float, access::read> tex; ulong md; };\n\
struct P { uint sw; uint sh; uint dw; uint dh; };\n\
kernel void present_dump(device const SrcEntry* s [[buffer(29)]], device uchar4* out [[buffer(28)]], \
constant P& p [[buffer(30)]], uint2 t [[thread_position_in_grid]]) {\n\
    if (t.x >= p.sw || t.y >= p.sh) return;\n\
    float4 c = clamp(s->tex.read(t), 0.0, 1.0);\n\
    out[ulong(t.y) * p.sw + t.x] = uchar4(rint(c * 255.0));\n}\n";

fn crc32(data: &[u8]) -> u32 {
    let mut c = 0xffff_ffffu32;
    for &b in data {
        c ^= u32::from(b);
        for _ in 0..8 {
            c = if c & 1 != 0 {
                0xedb8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
        }
    }
    !c
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for chunk in data.chunks(5552) {
        for &x in chunk {
            a += u32::from(x);
            b += a;
        }
        a %= 65521;
        b %= 65521;
    }
    (b << 16) | a
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], body: &[u8]) {
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    let start = out.len();
    out.extend_from_slice(kind);
    out.extend_from_slice(body);
    let crc = crc32(&out[start..]);
    out.extend_from_slice(&crc.to_be_bytes());
}

/// A PNG (8-bit RGBA, stored deflate) of `rgba` (`width * height * 4` bytes).
pub fn encode_png_rgba8(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    assert_eq!(rgba.len() as u64, u64::from(width) * u64::from(height) * 4);
    let row = width as usize * 4;
    let mut raw = Vec::with_capacity((row + 1) * height as usize);
    for y in 0..height as usize {
        raw.push(0);
        raw.extend_from_slice(&rgba[y * row..(y + 1) * row]);
    }
    let mut z = vec![0x78, 0x01];
    let mut blocks = raw.chunks(65535).peekable();
    while let Some(b) = blocks.next() {
        z.push(u8::from(blocks.peek().is_none()));
        z.extend_from_slice(&(b.len() as u16).to_le_bytes());
        z.extend_from_slice(&(!(b.len() as u16)).to_le_bytes());
        z.extend_from_slice(b);
    }
    z.extend_from_slice(&adler32(&raw).to_be_bytes());
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    out
}

/// Writes [`encode_png_rgba8`] to `path`.
pub fn write_png_rgba8(path: &Path, width: u32, height: u32, rgba: &[u8]) -> std::io::Result<()> {
    std::fs::write(path, encode_png_rgba8(width, height, rgba))
}

/// Reads what [`encode_png_rgba8`] wrote: `(width, height, rgba)`.
pub fn read_png_rgba8(png: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    let mut p = 8usize;
    let (mut w, mut h, mut idat) = (0u32, 0u32, Vec::new());
    while p + 8 <= png.len() {
        let len = u32::from_be_bytes(png[p..p + 4].try_into().ok()?) as usize;
        let kind = &png[p + 4..p + 8];
        let body = png.get(p + 8..p + 8 + len)?;
        match kind {
            b"IHDR" => {
                w = u32::from_be_bytes(body[0..4].try_into().ok()?);
                h = u32::from_be_bytes(body[4..8].try_into().ok()?);
            }
            b"IDAT" => idat.extend_from_slice(body),
            _ => {}
        }
        p += 12 + len;
    }
    // stored blocks only
    let mut z = idat.get(2..)?;
    let mut raw = Vec::new();
    loop {
        let last = z[0] & 1 != 0;
        let n = u16::from_le_bytes([z[1], z[2]]) as usize;
        raw.extend_from_slice(z.get(5..5 + n)?);
        z = &z[5 + n..];
        if last {
            break;
        }
    }
    let row = w as usize * 4;
    let mut out = Vec::with_capacity(row * h as usize);
    for y in 0..h as usize {
        out.extend_from_slice(raw.get(y * (row + 1) + 1..(y + 1) * (row + 1))?);
    }
    Some((w, h, out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_parses() {
        let s = DumpSpec::parse("300, 1322@/tmp/x").unwrap();
        assert_eq!(s.presents, [300, 1322]);
        assert_eq!(s.dir, PathBuf::from("/tmp/x"));
        assert!(DumpSpec::parse("300").is_none());
        assert!(DumpSpec::parse("@/tmp").is_none());
        assert!(DumpSpec::parse("x@/tmp").is_none());
    }

    #[test]
    fn png_round_trips_and_checksums() {
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
        assert_eq!(adler32(b"Wikipedia"), 0x11e6_0398);
        let (w, h) = (300u32, 250u32); // more than one 65535-byte block
        let px: Vec<u8> = (0..w * h * 4).map(|i| (i * 7 % 251) as u8).collect();
        let png = encode_png_rgba8(w, h, &px);
        assert_eq!(read_png_rgba8(&png), Some((w, h, px)));
    }
}
