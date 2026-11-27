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
}