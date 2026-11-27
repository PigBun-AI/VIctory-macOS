//! The shader cache reader and root signature builder shared by the survey
//! tests (`msc_survey.rs`, `gfx_survey.rs`). Cache entries are the user's
//! game-derived data: read in place, never copied; only aggregate numbers
//! leave the tests.
#![cfg(target_vendor = "apple")]
#![allow(dead_code)]

use std::path::{Path, PathBuf};

use victory_metal_back::msc::{
    Param, ParamKind, RootSignatureDesc, StaticSampler, Visibility, flags,
};

/// What the cache entry says the host binds.
#[derive(Default)]
pub struct Entry {
    pub stage: u32,
    pub dxil: Vec<u8>,
    pub user_data_dwords: u32,
    pub static_samplers: Vec<StaticSampler>,
    pub root_cbvs: Vec<u8>,
    pub root_srvs: Vec<u8>,
    pub root_uavs: Vec<u8>,
    pub memory: bool,
    pub memory_writes: bool,
    pub view_addresses: bool,
    pub view_marks: bool,
}

pub struct Reader<'a>(&'a [u8]);
impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let (h, r) = self.0.split_at_checked(n)?;
        self.0 = r;
        Some(h)
    }
    fn word(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn list(&mut self) -> Option<Vec<u8>> {
        let n = self.word()? as usize;
        Some(self.take(n)?.to_vec())
    }
}

pub enum Parsed {
    Entry(Box<Entry>),
    /// The translator refused the container; the cache keeps the reason.
    Refused,
    Unreadable,
}

pub fn parse(bytes: &[u8]) -> Parsed {
    if bytes.starts_with(b"VSHE") {
        return Parsed::Refused;
    }
    let Some(e) = parse_entry(bytes) else {
        return Parsed::Unreadable;
    };
    Parsed::Entry(Box::new(e))
}

pub fn parse_entry(bytes: &[u8]) -> Option<Entry> {
    let mut r = Reader(bytes);
    if r.take(4)? != b"VSH3" {
        return None;
    }
    let mut e = Entry {
        stage: r.word()?,
        dxil: r.list()?,
        ..Default::default()
    };
    let n = r.word()?;
    let mut max = 0;
    for _ in 0..n {
        let index = r.word()?;
        r.take(12)?; // slot, root type, dword
        max = max.max(index + 1);
    }
    e.user_data_dwords = max.next_multiple_of(4);
    let n = r.word()?;
    r.take(16 * n as usize)?; // the sampler words
    let n = r.word()?;
    for i in 0..n {
        let filter = r.word()?;
        let address = [r.word()?, r.word()?, r.word()?];
        let mip_lod_bias = f32::from_bits(r.word()?);
        let max_anisotropy = r.word()?;
        let comparison = r.word()?;
        let border = r.word()?;
        let min_lod = f32::from_bits(r.word()?);
        let max_lod = f32::from_bits(r.word()?);
        e.static_samplers.push(StaticSampler {
            filter,
            address,
            mip_lod_bias,
            max_anisotropy,
            comparison,
            border,
            min_lod,
            max_lod,
            register: i,
            space: 0,
            visibility: Visibility::All,
        });
    }
    e.root_cbvs = r.list()?;
    e.root_srvs = r.list()?;
    e.root_uavs = r.list()?;
    let n = r.word()?;
    r.take(8 * n as usize)?; // root SRV byte ends
    let bits = r.word()?;
    e.memory = bits & 1 != 0;
    e.memory_writes = bits & 2 != 0;
    e.view_addresses = bits & 4 != 0;
    e.view_marks = bits & 32 != 0;
    let n = r.word()?;
    r.take(8 * n as usize)?; // half views
    r.0.is_empty().then_some(e)
}

/// The stages in the cache entry's order: Ls, Hs, Es, Gs, Vs, Ps, Ms, Cs.
pub fn visibility(stage: u32) -> Visibility {
    match stage {
        1 => Visibility::Hull,
        3 => Visibility::Geometry,
        4 => Visibility::Vertex,
        5 => Visibility::Pixel,
        6 => Visibility::Mesh,
        _ => Visibility::All,
    }
}

pub const STAGE_NAMES: [&str; 8] = ["Ls", "Hs", "Es", "Gs", "Vs", "Ps", "Ms", "Cs"];

pub fn root_signature(e: &Entry) -> RootSignatureDesc {
    let vis = visibility(e.stage);
    let mut params = Vec::new();
    if e.user_data_dwords > 0 {
        params.push(Param {
            kind: ParamKind::Constants {
                register: 0,
                space: 0,
                num_32bit: e.user_data_dwords,
            },
            visibility: vis,
        });
    }
    let desc = |kind: fn(u32, u32) -> ParamKind, slots: &[u8], space: u32, out: &mut Vec<Param>| {
        for &s in slots {
            out.push(Param {
                kind: kind(u32::from(s), space),
                visibility: vis,
            });
        }
    };
    let cbv = |register, space| ParamKind::Cbv { register, space };
    let srv = |register, space| ParamKind::Srv { register, space };
    let uav = |register, space| ParamKind::Uav { register, space };
    desc(cbv, &e.root_cbvs, 1, &mut params);
    desc(srv, &e.root_srvs, 1, &mut params);
    desc(uav, &e.root_uavs, 1, &mut params);
    if e.memory {
        desc(srv, &[0], 2, &mut params);
    }
    if e.memory_writes {
        desc(uav, &[0], 2, &mut params);
    }
    if e.view_addresses {
        desc(srv, &[1], 2, &mut params);
    }
    if e.view_marks {
        // VIEW_MARKS = 2 (SRV), VIEW_WRITES = 1 (UAV), both in space 2.
        desc(srv, &[2], 2, &mut params);
        desc(uav, &[1], 2, &mut params);
    }
    let mut f = flags::HEAP_INDEXED | flags::SAMPLER_HEAP_INDEXED;
    if e.stage != 7 {
        f |= flags::ALLOW_INPUT_LAYOUT;
    }
    let mut samplers = e.static_samplers.clone();
    for s in &mut samplers {
        s.visibility = vis;
    }
    RootSignatureDesc {
        flags: f,
        params,
        static_samplers: samplers,
    }
}

pub fn files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            files(&p, out);
        } else {
            out.push(p);
        }
    }
}

pub fn percentile(v: &mut [f64], p: f64) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[((v.len() - 1) as f64 * p).round() as usize]
}

pub fn first_line(s: &str) -> String {
    let l = s.lines().next().unwrap_or("");
    l.chars().take(160).collect::<String>().replace('\t', " ")
}
