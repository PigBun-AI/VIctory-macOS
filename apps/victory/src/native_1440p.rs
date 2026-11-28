//! A 2560x1440 SDR display for the supported Series X retail image (GTA VI
//! 1.0.2.1): the title presents at 3840x2160 on Series X, its scene drawn at
//! 2560x1440 and scaled up by its own temporal pass. With 1440p asked for
//! (`VICTORY_NATIVE_1440P=1`, or `VICTORY_RENDER_1080P=1` for a 1920x1080
//! scene scaled to 1440p), the image's SDR display mode, the scanout
//! reservation, the temporal scene scale and the Settings viewport load are
//! patched before it is mapped: the title then makes a 1440p output, and no
//! 4K pass runs.
//!
//! Ported from DexNom's fork (DexNom/VIctory, `apps/victory/src/native_1440p.rs`,
//! GPL-3.0-or-later), its native 1440p part only. Every field is checked
//! against the stock bytes first: another build is refused, not patched.

use anyhow::{Context, Result, ensure};
use victory_formats::texture::{Dimension, Swizzle, Texture, TextureDesc};
use victory_loader::PreparedImage;
use victory_loader::render_scale::{self, MAXIMUM, PINNED_MAXIMUM, STOCK_MAXIMUM, STOCK_MINIMUM, scene_size};

const RETAIL_BUILD: &str = "b99f876b74d4a77f03af5f878223b27bd647b5c35a79d5decd9f716504239e93";
pub(super) const WIDTH: u32 = 2560;
pub(super) const HEIGHT: u32 = 1440;
const SDR_MODE: u32 = 0x0615_c100;
const STOCK_SDR_MODE: [u8; 16] = [0, 15, 0, 0, 112, 8, 0, 0, 60, 0, 0, 0, 1, 0, 0, 0];
const SCANOUT_RESERVATION: u32 = 0x0247_ad4a;
const TEMPORAL_SCALE: u32 = render_scale::MINIMUM;
const UI_VIEWPORT_LOAD: u32 = 0x0580_48d4;

/// Every range of the image `prepare` writes (what, RVA, length); the dynamic scale's `vmaxss` only when
/// pinned. (The runtime `render_scale::watch` detour rewrites the same eight bytes of that `vmaxss`, and
/// accepts the stock and the pinned form for that reason.) mac X13 pins this list against L3's sites
/// (`resolution::plan_asked`) in the tests below.
#[cfg(test)]
pub(super) fn written(pin: bool) -> Vec<(&'static str, u32, usize)> {
    let mut sites = vec![
        ("SDR display mode record", SDR_MODE, 16),
        ("scanout reservation", SCANOUT_RESERVATION, 5),
        ("temporal scene scale minimum", TEMPORAL_SCALE, 4),
        ("Settings preview viewport load", UI_VIEWPORT_LOAD, 8),
    ];
    if pin {
        sites.push(("temporal scene scale controller (vmaxss)", MAXIMUM, 8));
    }
    sites
}

/// Patches `image` for a 2560x1440 output; the scene is drawn at `scale` of
/// it (1.0: 2560x1440, 0.75: 1920x1080). With `pin` the scale is fixed there
/// (`victory_loader::render_scale`); without, `scale` is only the title's
/// minimum and its dynamic-resolution controller may go above it. Returns the
/// scene's size.
pub(super) fn prepare(image: &mut PreparedImage, scale: f32, pin: bool) -> Result<(u32, u32)> {
    ensure!(
        image.original_hash.to_hex().as_str() == RETAIL_BUILD,
        "1440p output is only known for the retail build {RETAIL_BUILD}"
    );

    let mode = field::<16>(image, SDR_MODE)?;
    ensure!(*mode == STOCK_SDR_MODE, "unexpected Series X SDR mode");
    mode[..4].copy_from_slice(&WIDTH.to_le_bytes());
    mode[4..8].copy_from_slice(&HEIGHT.to_le_bytes());

    // Two 64 KiB-aligned, tiled RGBA8 scanout buffers. The reservation is
    // rounded to 2 MiB, matching the title's allocation check.
    let texture = Texture::new(&TextureDesc {
        dimension: Dimension::Tex2d,
        width: WIDTH,
        height: HEIGHT,
        depth_or_array: 1,
        mips: 1,
        format: 28,
        samples: 1,
        swizzle: Swizzle::SW_64KB_D_X,
    })
    .map_err(|error| anyhow::anyhow!("1440p scanout layout: {error:?}"))?;
    let pool_size =
        u32::try_from((texture.size.next_multiple_of(64 * 1024) * 2).next_multiple_of(2 << 20))?;
    let reservation = field::<5>(image, SCANOUT_RESERVATION)?;
    ensure!(*reservation == [0xb9, 0, 0, 0, 4], "unexpected Series X scanout reservation");
    reservation[1..].copy_from_slice(&pool_size.to_le_bytes());

    ensure!((0.25..=1.0).contains(&scale), "render scale {scale} outside 0.25..=1.0");
    let minimum = field::<4>(image, TEMPORAL_SCALE)?;
    ensure!(*minimum == STOCK_MINIMUM, "unexpected Series X temporal scene scale");
    *minimum = scale.to_le_bytes();
    let (render_width, render_height) = scene_size(WIDTH, HEIGHT, scale);
    if pin {
        let maximum = field::<8>(image, MAXIMUM)?;
        ensure!(*maximum == STOCK_MAXIMUM, "unexpected Series X dynamic scene scale");
        *maximum = PINNED_MAXIMUM;
    }

    // Settings lays out its preview panels with a viewport setter of its
    // own that loads the legacy UI size (3840x2160) even when a 1440p UI
    // surface was made: it reads the selected SDR mode instead.
    let viewport = field::<8>(image, UI_VIEWPORT_LOAD)?;
    ensure!(
        *viewport == [0xc5, 0xfb, 0x10, 0x0d, 0x10, 0xf0, 0x13, 0x05],
        "unexpected Settings viewport size load"
    );
    let displacement = (SDR_MODE as i32) - (UI_VIEWPORT_LOAD as i32 + 8);
    viewport[4..].copy_from_slice(&displacement.to_le_bytes());
    Ok((render_width, render_height))
}

fn field<const N: usize>(image: &mut PreparedImage, rva: u32) -> Result<&mut [u8; N]> {
    let offset = image.info.rva_to_file_offset(rva).context("display profile field missing")? as usize;
    image
        .bytes
        .get_mut(offset..offset + N)
        .context("incomplete display profile field")?
        .try_into()
        .context("invalid display profile field length")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// With the retail executable named by `VICTORY_DISPLAY_PROFILE_TEST_IMAGE`
    /// (not part of the repository): the fields hold what a 1440p output
    /// needs, for each scene scale, pinned or not; without it, nothing is
    /// checked.
    #[test]
    fn patches_the_retail_image_for_1440p() {
        let Some(path) = std::env::var_os("VICTORY_DISPLAY_PROFILE_TEST_IMAGE") else {
            return;
        };
        let bytes = std::fs::read(path).unwrap();
        for (scale, pin, scene) in [
            (1.0, false, (2560, 1440)),
            (0.75, false, (1920, 1080)),
            (0.75, true, (1920, 1080)),
            (2.0 / 3.0, true, (1706, 960)),
            (0.5, true, (1280, 720)),
            (1.0, true, (2560, 1440)),
        ] {
            let mut image = victory_loader::prepare(&bytes).unwrap();
            assert_eq!(prepare(&mut image, scale, pin).unwrap(), scene);
            let mode = *field::<16>(&mut image, SDR_MODE).unwrap();
            assert_eq!(mode[..4], WIDTH.to_le_bytes());
            assert_eq!(mode[4..8], HEIGHT.to_le_bytes());
            let minimum = f32::from_le_bytes(*field::<4>(&mut image, TEMPORAL_SCALE).unwrap());
            assert_eq!((WIDTH as f32 * minimum) as u32, scene.0);
            let maximum = *field::<8>(&mut image, MAXIMUM).unwrap();
            assert_eq!(maximum, if pin { PINNED_MAXIMUM } else { STOCK_MAXIMUM }, "scale {scale} pin {pin}");
            let load = *field::<8>(&mut image, UI_VIEWPORT_LOAD).unwrap();
            let disp = i32::from_le_bytes(load[4..].try_into().unwrap());
            assert_eq!(UI_VIEWPORT_LOAD.wrapping_add(8).wrapping_add_signed(disp), SDR_MODE);
            // Patched once, the image is no longer the stock one: refused.
            assert!(prepare(&mut image, scale, pin).is_err());
        }
        // Adversarial: a stock byte of the dynamic scale's `vmaxss` changed
        // (another build): pinning is refused, not written over it.
        let mut image = victory_loader::prepare(&bytes).unwrap();
        field::<8>(&mut image, MAXIMUM).unwrap()[2] = 0x5d;
        assert!(prepare(&mut image, 0.75, true).is_err());
        // Out of range scales are refused.
        let mut image = victory_loader::prepare(&bytes).unwrap();
        assert!(prepare(&mut image, 1.5, true).is_err());
    }
    // ---- mac X13: native 1440p and L3 side by side ---------------------------------------------

    use victory_loader::mac::output_chain::{Decision, Env, decide};
    use victory_loader::mac::resolution::{self as l3, Asked, Site};

    type Range = (String, u32, usize);

    fn l3_ranges(chain: &Decision, pool_skip: bool, upscaler: Option<u32>, scale: Option<f32>) -> Vec<Range> {
        let scale = if chain.l3_scale { scale } else { None };
        l3::plan_asked(chain.l3_output, pool_skip, scale, upscaler)
            .iter()
            .flat_map(|(group, _)| group.iter().map(|(site, code)| (format!("L3 {}", site.what), site.at as u32, code.len())))
            .collect()
    }

    fn overlaps(a: &[Range]) -> Vec<(String, String)> {
        let mut found = Vec::new();
        for (i, (an, at, al)) in a.iter().enumerate() {
            for (bn, bt, bl) in &a[i + 1..] {
                if *at < bt + *bl as u32 && *bt < at + *al as u32 {
                    found.push((an.clone(), bn.clone()));
                }
            }
        }
        found
    }

    /// The whole table of switches (profile x native x scale x output x pool check x upscaler): in every
    /// combination, the sites the plan writes (L3's, in the mapped image) and the native ones (in the
    /// prepared image) are pairwise disjoint, and the native 1440p chain never comes with L3's output or scale.
    #[test]
    fn native_and_l3_sites_never_overlap_in_any_switch_combination() {
        let mut both = 0;
        let mut native_on = 0;
        for profile in [None, Some("fidelity"), Some("perf")] {
            for native in [None, Some("0"), Some("1")] {
                for r1080 in [None, Some("1")] {
                    for scale in [None, Some("0.667"), Some("0.5")] {
                        for output in [None, Some("0"), Some("720"), Some("1280x720"), Some("2560x1440"), Some("3840x2160")] {
                            for pool_skip in [false, true] {
                                for upscaler in [None, Some(6), Some(7)] {
                                    let env = Env {
                                        profile,
                                        output,
                                        native_1440p: native,
                                        render_1080p: r1080,
                                        render_scale: scale,
                                        mac: true,
                                        series_x: true,
                                    };
                                    let chain = decide(&env);
                                    let mut ranges = l3_ranges(&chain, pool_skip, upscaler, scale.and_then(|s| s.parse().ok()));
                                    let l3_count = ranges.len();
                                    if chain.native_1440p {
                                        native_on += 1;
                                        assert_eq!(chain.l3_output, Asked::Title, "{env:?}");
                                        assert!(!ranges.iter().any(|r| r.1 == l3::OWN_SCALE.at as u32), "L3's scale site with native 1440p: {env:?}");
                                        assert!(!ranges.iter().any(|r| r.1 == l3::OUTPUT.at as u32), "L3's output with native 1440p: {env:?}");
                                        let pin = true; // the superset: the controller site too
                                        ranges.extend(written(pin).into_iter().map(|(n, at, l)| (format!("native {n}"), at, l)));
                                        if l3_count > 0 {
                                            both += 1;
                                        }
                                    }
                                    assert_eq!(overlaps(&ranges), [], "{env:?} pool_skip {pool_skip} upscaler {upscaler:?}");
                                    if profile == Some("fidelity") {
                                        assert!(!chain.native_1440p, "fidelity: native 1440p off ({env:?})");
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        assert!(native_on > 0, "the table has native 1440p on");
        assert!(both > 0, "the table has native 1440p and an L3 site (the clamp, pool check, upscaler) written in one run");
    }

    /// The finding behind dropping L3's scale when native 1440p is on: of every site L3 can write and every
    /// range native 1440p writes, exactly one pair overlaps: L3's scale immediate (+0x28f1e78, 10 bytes
    /// `mov dword [rcx+90h], imm32`) holds the native minimum (+0x28f1e7e, the imm32) in its last four bytes.
    /// (The REPORT's first byte comparison called the two groups disjoint: it compared the output and pool
    /// check sites, not the scale site.)
    #[test]
    fn the_only_overlap_of_the_full_site_lists_is_the_scale_immediate() {
        let mut all: Vec<Range> = Vec::new();
        for site in [&l3::OUTPUT, &l3::POOL_CHECK, &l3::OWN_SCALE, &l3::DIV_ZERO, &l3::DIV_TRAMP, &l3::PICKER] {
            all.push((format!("L3 {}", site.what), site.at as u32, site.bytes.len()));
        }
        all.extend(written(true).into_iter().map(|(n, at, l)| (format!("native {n}"), at, l)));
        // The title's frame rate loads (VICTORY_FRAME_INTERVAL_MS, the chain's first commit) are written to the mapped
        // image as well: two more ranges that must stay clear of both (the second lies 0x32 bytes after L3's output site).
        for (i, site) in victory_loader::frame_rate::SITES.iter().enumerate() {
            all.push((format!("frame rate load {i}"), site.at as u32, site.bytes.len()));
        }
        assert_eq!(
            overlaps(&all),
            [("L3 the title's own upscaler's render scale".to_string(), "native temporal scene scale minimum".to_string())]
        );
        assert_eq!(l3::OWN_SCALE.at + 6, TEMPORAL_SCALE as usize, "the immediate of the L3 site is the native minimum");
        assert_eq!(l3::OWN_SCALE.bytes[6..], STOCK_MINIMUM);
        // The pool check and the scanout reservation are two different places of one function area.
        assert!((SCANOUT_RESERVATION as usize + 5) < l3::POOL_CHECK.at);
        // The settings' size site and the SDR record: different bytes, different sections.
        assert!(l3::OUTPUT.at + l3::OUTPUT.bytes.len() < UI_VIEWPORT_LOAD as usize);
    }

    /// The retail image named by `VICTORY_DISPLAY_PROFILE_TEST_IMAGE`, else `$VICTORY_GAME_DIR/game_scarlett_master.exe`
    /// (read only); `None`, said, when the machine has neither.
    fn title_image() -> Option<Vec<u8>> {
        let path = std::env::var_os("VICTORY_DISPLAY_PROFILE_TEST_IMAGE")
            .map(std::path::PathBuf::from)
            .or_else(|| std::env::var_os("VICTORY_GAME_DIR").map(|d| std::path::Path::new(&d).join("game_scarlett_master.exe")))
            .filter(|p| p.is_file());
        match path {
            Some(path) => Some(std::fs::read(path).unwrap()),
            None => {
                eprintln!("native_1440p byte tests: no title image (VICTORY_DISPLAY_PROFILE_TEST_IMAGE or VICTORY_GAME_DIR): skipped");
                None
            }
        }
    }

    /// The bytes at `rva`: where the section has them in the file. The divide clamp's padding
    /// (+0x5f40966) is in the raw data past the section's virtual size (the title's `.text` ends there),
    /// which `rva_to_file_offset` leaves out and the loader's mapping, page by page, still holds.
    fn at(image: &mut PreparedImage, rva: u32, len: usize) -> &mut [u8] {
        let offset = image
            .info
            .rva_to_file_offset(rva)
            .or_else(|| {
                image.info.sections.iter().find_map(|s| {
                    let delta = rva.checked_sub(s.rva)?;
                    (delta < s.file_size).then_some(s.file_offset + delta)
                })
            })
            .unwrap_or_else(|| panic!("rva {rva:#x} is not in the image")) as usize;
        &mut image.bytes[offset..offset + len]
    }

    /// Applies the L3 plan to the prepared image the way `resolution::write_together` does on the mapped one: every
    /// site's bytes are compared first (a mismatch is what `Patched::Differs` is), then written.
    fn apply_l3(image: &mut PreparedImage, groups: &[(&'static Site, Vec<u8>)]) -> Result<(), String> {
        for (site, code) in groups {
            if at(image, site.at as u32, site.bytes.len()) != site.bytes {
                return Err(format!("{} differs", site.what));
            }
            assert!(code.len() <= site.bytes.len());
        }
        for (site, code) in groups {
            at(image, site.at as u32, code.len()).copy_from_slice(code);
        }
        Ok(())
    }

    /// With the title image of this machine (read only; `VICTORY_GAME_DIR` or `VICTORY_DISPLAY_PROFILE_TEST_IMAGE`):
    /// (1) every L3 site and every native field holds its stock bytes; (2) for the switch combinations that turn
    /// native 1440p on, the native image keeps every L3 site's stock bytes (so each L3 group is written, not refused) and L3's
    /// writes leave the native fields as they were; (3) fidelity writes nothing; (4) L3's scale site on top of
    /// the native image is refused, which is why it is dropped.
    #[test]
    fn the_native_and_l3_bytes_on_the_title_image() {
        let Some(bytes) = title_image() else { return };
        println!("native_1440p byte tests: title image of {} bytes", bytes.len());

        // (1) stock bytes
        let mut stock = victory_loader::prepare(&bytes).unwrap();
        assert_eq!(stock.original_hash.to_hex().as_str(), RETAIL_BUILD);
        for site in [&l3::OUTPUT, &l3::POOL_CHECK, &l3::OWN_SCALE, &l3::DIV_ZERO, &l3::DIV_TRAMP, &l3::PICKER] {
            assert_eq!(at(&mut stock, site.at as u32, site.bytes.len()), site.bytes, "L3 site {}", site.what);
        }
        for site in &victory_loader::frame_rate::SITES {
            assert_eq!(at(&mut stock, site.at as u32, site.bytes.len()), site.bytes, "frame rate load at {:#x}", site.at);
        }
        assert_eq!(at(&mut stock, SDR_MODE, 16), STOCK_SDR_MODE);
        assert_eq!(at(&mut stock, SCANOUT_RESERVATION, 5), [0xb9, 0, 0, 0, 4]);
        assert_eq!(at(&mut stock, TEMPORAL_SCALE, 4), STOCK_MINIMUM);
        assert_eq!(at(&mut stock, MAXIMUM, 8), STOCK_MAXIMUM);
        assert_eq!(at(&mut stock, UI_VIEWPORT_LOAD, 8), [0xc5, 0xfb, 0x10, 0x0d, 0x10, 0xf0, 0x13, 0x05]);
        // The display mode table the record is the fifth entry of (1280x720 ... 3840x2160 at 60 Hz).
        let table = at(&mut stock, SDR_MODE - 0x40, 0x50).to_vec();
        for (i, (w, h)) in [(1280u32, 720u32), (1664, 936), (1920, 1080), (2560, 1440), (3840, 2160)].into_iter().enumerate() {
            assert_eq!(table[i * 16..i * 16 + 4], w.to_le_bytes(), "mode {i} width");
            assert_eq!(table[i * 16 + 4..i * 16 + 8], h.to_le_bytes(), "mode {i} height");
            assert_eq!(table[i * 16 + 8..i * 16 + 12], 60u32.to_le_bytes(), "mode {i} rate");
        }
        drop(stock);

        // (2) both on, for a few combinations (an image per combination: the native patch is written once)
        let cases: [(&str, Env, bool, Option<u32>); 4] = [
            ("perf default + pool check skip", Env { profile: Some("perf"), mac: true, series_x: true, ..Env::default() }, true, None),
            ("perf, 1080p scale + upscaler 7", Env { profile: Some("perf"), render_1080p: Some("1"), mac: true, series_x: true, ..Env::default() }, false, Some(7)),
            ("perf, scale 0.667 pinned + both", Env { profile: Some("perf"), render_scale: Some("0.667"), mac: true, series_x: true, ..Env::default() }, true, Some(6)),
            ("perf, native asked over an output size", Env { profile: Some("perf"), native_1440p: Some("1"), output: Some("1280x720"), mac: true, series_x: true, ..Env::default() }, true, Some(7)),
        ];
        for (name, env, pool_skip, upscaler) in cases {
            let chain = decide(&env);
            assert!(chain.native_1440p, "{name}");
            let render = super::super::RenderScale::from(env.render_scale, env.render_1080p, None)
                .unwrap_or(super::super::RenderScale { scale: 1.0, pin: false });
            let mut image = victory_loader::prepare(&bytes).unwrap();
            prepare(&mut image, render.scale, render.pin).unwrap();
            let native: Vec<(u32, Vec<u8>)> = written(render.pin).into_iter().map(|(_, rva, len)| (rva, at(&mut image, rva, len).to_vec())).collect();
            let plan = l3::plan_asked(chain.l3_output, pool_skip, None, upscaler);
            let mut l3_writes = 0;
            for (group, value) in &plan {
                apply_l3(&mut image, group).unwrap_or_else(|why| panic!("{name}: group {value}: {why}"));
                l3_writes += group.len();
            }
            assert!(l3_writes >= 3, "{name}: L3 wrote something next to the native fields ({l3_writes} sites)");
            for (rva, was) in native {
                let len = was.len();
                assert_eq!(at(&mut image, rva, len), was, "{name}: the native field at {rva:#x} changed under L3's writes");
            }
            // The mode record is 2560x1440 and the scanout reservation sized for it.
            assert_eq!(at(&mut image, SDR_MODE, 8), [0x00, 0x0a, 0x00, 0x00, 0xa0, 0x05, 0x00, 0x00], "{name}");
            let reservation = at(&mut image, SCANOUT_RESERVATION, 5).to_vec();
            assert_eq!(reservation[0], 0xb9);
            let pool = u32::from_le_bytes(reservation[1..].try_into().unwrap());
            assert_eq!(pool % (2 << 20), 0, "{name}: whole 2 MiB");
            assert!(pool >= 2 * 2560 * 1440 * 4 && pool < 64 << 20, "{name}: pool {pool:#x} holds two 1440p buffers and is below the 4K pool of 64 MB");
        }

        // (3) fidelity: nothing is planned, and the native fields stay as the stock image has them
        for env in [
            Env { profile: Some("fidelity"), native_1440p: Some("1"), render_1080p: Some("1"), output: Some("0"), mac: true, series_x: true, ..Env::default() },
            Env { profile: Some("fidelity"), mac: true, series_x: true, ..Env::default() },
        ] {
            let chain = decide(&env);
            assert!(!chain.native_1440p);
            assert!(l3::plan_asked(chain.l3_output, false, None, None).is_empty(), "fidelity patches no site");
        }

        // (4) the dropped combination: L3's scale site over the native image is refused
        let mut image = victory_loader::prepare(&bytes).unwrap();
        prepare(&mut image, 1.0, false).unwrap();
        let scale = l3::plan_asked(Asked::Title, false, Some(0.75), None);
        let (group, _) = scale.iter().find(|(g, _)| g.iter().any(|(s, _)| s.at == l3::OWN_SCALE.at)).expect("a scale group");
        assert!(apply_l3(&mut image, group).is_err(), "the native minimum (1.0) is in the bytes L3's scale site compares");
        assert_eq!(at(&mut image, TEMPORAL_SCALE, 4), 1.0f32.to_le_bytes());
    }
}
