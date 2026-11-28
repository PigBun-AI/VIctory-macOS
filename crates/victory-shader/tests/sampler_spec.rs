//! S-G-009 (docs/mac/PATCHES.md G12): the console's sampler words decoded to a D3D12 static sampler, against the
//! register layout and the D3D12 definitions, over every value of every field (not the four samples the unit tests use).
//!
//! References:
//! * The words are the GPU's public `SQ_IMG_SAMP_WORD0..3` (GFX10): AMD RDNA ISA / Mesa `src/amd/common/sid.h` at
//!   commit ec5abcdd (`S_008F30_CLAMP_X/Y/Z` bits 0-8, `MAX_ANISO_RATIO` 9-11, `DEPTH_COMPARE_FUNC` 12-14,
//!   `FILTER_MODE` 29-30; `S_008F34_MIN_LOD` 0-11, `MAX_LOD` 12-23; `S_008F38_LOD_BIAS` 0-13 (signed, 8 fractional bits),
//!   `XY_MAG_FILTER` 20-21, `XY_MIN_FILTER` 22-23, `MIP_FILTER` 26-27; `S_008F3C_BORDER_COLOR_TYPE` 30-31) and Mesa
//!   `radv_sampler.c` for how those are filled from Vulkan/D3D-like state: clamp modes WRAP 0, MIRROR 1,
//!   CLAMP_LAST_TEXEL 2, MIRROR_ONCE_LAST_TEXEL 3, CLAMP_HALF_BORDER 4, MIRROR_ONCE_HALF_BORDER 5, CLAMP_BORDER 6,
//!   MIRROR_ONCE_BORDER 7; depth compare NEVER 0 .. ALWAYS 7 (same order as D3D12's, which count from 1); filters
//!   POINT 0, BILINEAR 1, ANISO_POINT 2, ANISO_BILINEAR 3; mip filter NONE 0, POINT 1, LINEAR 2; border TRANSPARENT 0,
//!   OPAQUE_BLACK 1, OPAQUE_WHITE 2, REGISTER 3; filter mode BLEND 0, MIN 1, MAX 2.
//! * D3D12: `D3D12_FILTER` encoding (`D3D12_ENCODE_BASIC_FILTER`: reduction << 7 | min << 4 | mag << 2 | mip;
//!   `D3D12_ENCODE_ANISOTROPIC_FILTER`: 0x40 | basic filter), `D3D12_TEXTURE_ADDRESS_MODE` (WRAP 1, MIRROR 2, CLAMP 3,
//!   BORDER 4, MIRROR_ONCE 5), `D3D12_COMPARISON_FUNC` (NEVER 1 .. ALWAYS 8), `D3D12_STATIC_BORDER_COLOR`
//!   (TRANSPARENT_BLACK 0, OPAQUE_BLACK 1, OPAQUE_WHITE 2), `D3D12_STATIC_SAMPLER_DESC` (`MaxAnisotropy` 1..16).
use victory_shader::emit::StaticSampler;

/// An encoder of the register fields (Mesa's S_008F3x macros), the way a driver writes them.
struct Samp {
    clamp: [u32; 3],
    aniso_log2: u32,
    compare: u32,
    reduction: u32,
    min_lod: u32,
    max_lod: u32,
    bias: i32,
    mag: u32,
    min: u32,
    mip: u32,
    border: u32,
}

impl Samp {
    fn words(&self) -> [u32; 4] {
        [
            self.clamp[0] | self.clamp[1] << 3 | self.clamp[2] << 6 | self.aniso_log2 << 9 | self.compare << 12 | self.reduction << 29,
            self.min_lod & 0xfff | (self.max_lod & 0xfff) << 12,
            (self.bias as u32 & 0x3fff) | self.mag << 20 | self.min << 22 | self.mip << 26,
            self.border << 30,
        ]
    }
}

fn base() -> Samp {
    Samp { clamp: [0; 3], aniso_log2: 0, compare: 0, reduction: 0, min_lod: 0, max_lod: 0xfff, bias: 0, mag: 0, min: 0, mip: 2, border: 0 }
}

#[test]
fn every_address_mode_of_every_axis() {
    // D3D12 has no half-border modes: the nearest are CLAMP (4) and MIRROR_ONCE (5)
    let d3d = [1, 2, 3, 5, 3, 5, 4, 5];
    for mode in 0..8u32 {
        for axis in 0..3 {
            let mut s = base();
            s.clamp = [0, 0, 0];
            s.clamp[axis] = mode;
            let got = StaticSampler::from_words(s.words()).address;
            let mut want = [1, 1, 1];
            want[axis] = d3d[mode as usize];
            assert_eq!(got, want, "mode {mode} on axis {axis}");
        }
    }
    // the modes that have a D3D12 form map exactly
    for (mode, d) in [(0u32, 1u32), (1, 2), (2, 3), (3, 5), (6, 4)] {
        let mut s = base();
        s.clamp = [mode; 3];
        assert_eq!(StaticSampler::from_words(s.words()).address, [d; 3]);
    }
}

#[test]
fn every_comparison_function_counts_from_one_in_d3d12() {
    for f in 0..8 {
        let mut s = base();
        s.compare = f;
        assert_eq!(StaticSampler::from_words(s.words()).comparison, f + 1);
    }
}

#[test]
fn anisotropy_is_two_to_the_ratio_and_stops_at_16() {
    for r in 0..8 {
        let mut s = base();
        s.aniso_log2 = r;
        assert_eq!(StaticSampler::from_words(s.words()).max_anisotropy, 1 << r.min(4), "ratio field {r}");
    }
}

#[test]
fn every_filter_combination_is_the_d3d12_filter_of_its_name() {
    for reduction in 0..3u32 {
        // BLEND, MIN, MAX in the register; STANDARD 0, MINIMUM 2, MAXIMUM 3 in D3D12 (1 is COMPARISON, which the
        // sampling instruction chooses: `comparison samplers` are or-ed in by the lowering)
        let d3d_reduction = [0u32, 2, 3][reduction as usize];
        for mag in 0..4u32 {
            for min in 0..4u32 {
                for mip in 0..3u32 {
                    let mut s = base();
                    s.reduction = reduction;
                    (s.mag, s.min, s.mip) = (mag, min, mip);
                    let got = StaticSampler::from_words(s.words()).filter;
                    let aniso = mag >= 2 || min >= 2;
                    let linear_mip = u32::from(mip == 2);
                    let want = if aniso {
                        // D3D12_ENCODE_ANISOTROPIC_FILTER: 0x40 | MIN LINEAR | MAG LINEAR | mip; the point-mip form is 0x54
                        d3d_reduction << 7 | 0x40 | 0x14 | linear_mip
                    } else {
                        d3d_reduction << 7 | (min & 1) << 4 | (mag & 1) << 2 | linear_mip
                    };
                    assert_eq!(got, want, "reduction {reduction} mag {mag} min {min} mip {mip}");
                }
            }
        }
    }
    // the well-known values of the D3D12 header
    for (mag, min, mip, d3d12) in [(0u32, 0u32, 1u32, 0x00u32), (0, 0, 2, 0x01), (1, 0, 1, 0x04), (1, 1, 1, 0x14), (1, 1, 2, 0x15), (3, 3, 2, 0x55), (0, 1, 1, 0x10)] {
        let mut s = base();
        (s.mag, s.min, s.mip) = (mag, min, mip);
        assert_eq!(StaticSampler::from_words(s.words()).filter, d3d12);
    }
}

#[test]
fn lod_fields_are_unsigned_4_8_and_bias_is_signed_5_8() {
    for v in [0u32, 1, 255, 256, 257, 0x7ff, 0x800, 0xffe, 0xfff] {
        let mut s = base();
        (s.min_lod, s.max_lod) = (v, 0xfff - v / 2);
        let d = StaticSampler::from_words(s.words());
        assert_eq!((d.min_lod, d.max_lod), (v as f32 / 256.0, (0xfff - v / 2) as f32 / 256.0), "lod {v}");
    }
    for bias in [-8192i32, -4097, -256, -1, 0, 1, 255, 256, 4096, 8191] {
        let mut s = base();
        s.bias = bias;
        assert_eq!(StaticSampler::from_words(s.words()).mip_lod_bias, bias as f32 / 256.0, "bias {bias}");
    }
}

/// Without a mip filter only the base level is sampled: D3D12 says that with min = max = 0 LOD.
#[test]
fn no_mip_filter_samples_the_base_level_only() {
    let mut s = base();
    (s.mip, s.min_lod, s.max_lod) = (0, 0x300, 0xa00);
    let d = StaticSampler::from_words(s.words());
    assert_eq!((d.min_lod, d.max_lod), (0.0, 0.0));
    s.mip = 1;
    let d = StaticSampler::from_words(s.words());
    assert_eq!((d.min_lod, d.max_lod), (3.0, 10.0));
}

#[test]
fn border_colours() {
    for (t, d3d) in [(0u32, 0u32), (1, 1), (2, 2), (3, 0)] {
        let mut s = base();
        s.border = t;
        assert_eq!(StaticSampler::from_words(s.words()).border, d3d, "type {t}");
    }
}

/// The fields that no value of the others may disturb: every word with all other fields at random values.
#[test]
fn fields_do_not_touch_each_other() {
    let mut x = 0x2545_f491_4f6c_dd1du64;
    let mut next = move || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        (x >> 11) as u32
    };
    for _ in 0..20_000 {
        let w = [next(), next(), next(), next()];
        let d = StaticSampler::from_words(w);
        // the bits no field of the decoder reads (word 0: 15-28 and 31; word 1: 24-31; word 2: 14-19, 24-25, 28-31;
        // word 3: 0-29, the border colour pointer) change nothing
        let free = [0x9fff_8000u32, 0xff00_0000, 0xf30f_c000, 0x3fff_ffff];
        let mut v = w;
        for i in 0..4 {
            v[i] = w[i] & !free[i] | next() & free[i];
        }
        assert_eq!(StaticSampler::from_words(v), d, "{w:x?} vs {v:x?}");
    }
}
