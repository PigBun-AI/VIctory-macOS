//! Two switches, both off unless set, for a main-scene value that is an
//! infinity or NaN and turns the tone-mapping pass black (reports\sun-core-20261127.md:
//! one Inf channel in RT +304 makes `5970ee6dbc47ec61` output all black; a finite
//! 65504 outputs white).
//!
//! * `VICTORY_TM_CLAMP_HDR=1` (A): in the pixel program whose container hashes
//!   (blake3, first 16 hex) to [`TONEMAP_PS`], every image sample / load result
//!   (all four channels) is made NaN -> 0 and clamped to +-65504. SCOPE: the
//!   translator has no per-texture hook, so this covers *every* image read of that
//!   one program (its LUTs and +304 alike), not other shaders.
//! * `VICTORY_EXPORT_F16_SAT` (B, default ON, `0` turns it off): in `Lower::export`, a float colour export
//!   is NaN -> 0 and clamped to +-65504 before it is stored, for every pixel
//!   program (the host's RGBA16F targets overflow to Inf otherwise).
//!
//! Both names are literals in this crate's sources, so `build.rs` lists them and
//! `translation_fingerprint` hashes their values into the shader cache key.
use super::*;
use std::cell::Cell;

/// The tone-mapping pixel program (container hash).
pub const TONEMAP_PS: &str = "5970ee6dbc47ec61";

thread_local! { static CLAMP_SAMPLES: Cell<bool> = const { Cell::new(false) }; }

pub(in crate::emit) struct Scope(bool);
impl Drop for Scope {
    fn drop(&mut self) {
        CLAMP_SAMPLES.set(self.0);
    }
}

/// `value` is exactly "1" (after trimming). Pure, for testing.
pub(in crate::emit) fn on(value: Option<&str>) -> bool {
    value.is_some_and(|v| v.trim() == "1")
}

/// Whether A applies to a program: switch on, a pixel program, hash matches. Pure given its inputs.
pub(in crate::emit) fn wanted(switch: Option<&str>, bytes: &[u8], stage: HwStage) -> bool {
    on(switch)
        && stage == HwStage::Ps
        && blake3::hash(bytes).to_hex()[..16].eq_ignore_ascii_case(TONEMAP_PS)
}

pub(in crate::emit) fn scope(bytes: &[u8], stage: HwStage) -> Scope {
    let switch = std::env::var("VICTORY_TM_CLAMP_HDR").ok();
    Scope(CLAMP_SAMPLES.replace(wanted(switch.as_deref(), bytes, stage)))
}

pub(in crate::emit) fn samples_clamped() -> bool {
    CLAMP_SAMPLES.get()
}

/// B on unless the value is exactly "0" (after trimming): unset means on, matching the
/// Xbox export's saturation. Pure, for testing.
pub(in crate::emit) fn export_sat_for(value: Option<&str>) -> bool {
    value.map(str::trim) != Some("0")
}

/// B: `VICTORY_EXPORT_F16_SAT` (default on; "0" turns it off). Also part of the shader cache key.
pub fn export_sat() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| export_sat_for(std::env::var("VICTORY_EXPORT_F16_SAT").ok().as_deref()))
}

impl Lower<'_> {
    /// `f` with NaN -> 0 and -Inf/+Inf and anything beyond +-65504 clamped to +-65504
    /// (the NaN is replaced first: DXIL's FMax/FMin return the other operand for a NaN).
    pub(in crate::emit) fn sat_f16(&mut self, f: V) -> V {
        let c = self.cur;
        let ordered = self.s.m.cmp(c, pred::OEQ, f, f);
        let zero = self.s.m.f32(0);
        let f = self.s.m.select(c, ordered, f, zero);
        let lo = self.s.m.f32((-65504.0f32).to_bits());
        let hi = self.s.m.f32(65504.0f32.to_bits());
        let f = self.binary(35, f, lo);
        self.binary(36, f, hi)
    }
}

#[test]
fn switches_parse() {
    assert!(!on(None));
    assert!(!on(Some("0")));
    assert!(on(Some(" 1 ")));
}

#[test]
fn export_sat_is_on_unless_zero() {
    assert!(export_sat_for(None));
    assert!(export_sat_for(Some("1")));
    assert!(export_sat_for(Some("")));
    assert!(!export_sat_for(Some("0")));
    assert!(!export_sat_for(Some(" 0 ")));
}

#[test]
fn only_the_tonemap_program_with_the_switch_is_wanted() {
    let bytes = b"not the tonemap";
    assert!(!wanted(Some("1"), bytes, HwStage::Ps));
    assert!(!wanted(None, bytes, HwStage::Ps));
}

/// The tone-mapping container, as `pc_trace_tests::pc_trace_tonemap` reads it.
#[cfg(test)]
fn game_tonemap_container() -> Vec<u8> {
    use std::io::{Read, Seek, SeekFrom};
    let dir = std::env::var_os("VICTORY_GAME_DIR").expect("VICTORY_GAME_DIR");
    let mut f = std::fs::File::open(std::path::Path::new(&dir).join("shaders.rpf")).unwrap();
    f.seek(SeekFrom::Start(0x6e0538d)).unwrap();
    let mut h = [0; 12];
    f.read_exact(&mut h).unwrap();
    let u = |s: &[u8]| u32::from_le_bytes(s.try_into().unwrap()) as usize;
    assert_eq!(u(&h[..4]), 0xddce7);
    let mut packed = vec![0; u(&h[4..8]) - 12];
    f.read_exact(&mut packed).unwrap();
    let mut bytes = vec![0; u(&h[8..]) + 1];
    let mut z = zlib_rs::Inflate::new(false, 15);
    z.decompress(&packed, &mut bytes, zlib_rs::InflateFlush::NoFlush).unwrap();
    bytes.truncate(z.total_out() as usize);
    assert_eq!(&blake3::hash(&bytes).to_hex()[..16], TONEMAP_PS);
    bytes
}

/// A on / off on the real container: the switch changes only that program's DXIL (the
/// sample results get the clamp), and writes both to `VICTORY_SUNCLAMP_OUT` for the WARP
/// fixture (`reports\sun-core-fixture-20261127`, `PROBE_DXIL=`: Inf in +304 is white with A,
/// black without).
#[test]
#[ignore = "needs the game (VICTORY_GAME_DIR) and its own process (sets VICTORY_TM_CLAMP_HDR)"]
fn tonemap_clamp_changes_only_with_the_switch() {
    let bytes = game_tonemap_container();
    unsafe { std::env::remove_var("VICTORY_TM_CLAMP_HDR") };
    let off = crate::emit::translate(&bytes).unwrap();
    unsafe { std::env::set_var("VICTORY_TM_CLAMP_HDR", "1") };
    let on = crate::emit::translate(&bytes).unwrap();
    assert_ne!(off.dxil, on.dxil, "the switch changed nothing");
    unsafe { std::env::set_var("VICTORY_TM_CLAMP_HDR", "0") };
    assert_eq!(off.dxil, crate::emit::translate(&bytes).unwrap().dxil);
    if let Some(dir) = std::env::var_os("VICTORY_SUNCLAMP_OUT") {
        let dir = std::path::PathBuf::from(dir);
        std::fs::write(dir.join("tonemap-off.dxil"), &off.dxil).unwrap();
        std::fs::write(dir.join("tonemap-on.dxil"), &on.dxil).unwrap();
    }
}
