//! `VICTORY_LOG_RENDER_SIZE=1` (docs/mac/PATCHES.md, P2): says at what size
//! the title really renders its scene, for the output size it runs at. Off
//! unless set; with it off every hook is one atomic load and nothing is
//! recorded or changed.
//!
//! The title's internal size is not a setting it writes anywhere we can read
//! (its render size is the output size times its upscaler's scale, computed
//! in its own code), so it is read off what the title does, two ways:
//!
//! 1. **Draws per viewport** (the verdict). Every draw the title records is
//!    counted under the viewport it was recorded with (`SetViewports`, the
//!    first one, per recording thread). At each present the counts of the
//!    frame are taken; the *internal size* of that frame is the viewport with
//!    the most draws among those that are
//!    - **of the output's aspect ratio** (within 1 %): shadow maps and
//!      cube/atlas targets are square or odd-shaped and drop out here, and
//!    - **smaller than the output** (by area): the upscaler's input is the
//!      only large scene pass below the output; the output-sized passes (the
//!      upscaler's own result, post effects and the interface drawn at the
//!      output size) are counted and reported as `at output`, not as the
//!      internal size. When nothing smaller has any draws the internal size
//!      is the output (a render scale of 1.0).
//!    Half-size passes (ambient occlusion, bloom) are smaller still but have
//!    far fewer draws than the geometry passes at the internal size.
//! 2. **Render targets created** (the corroboration). Each new 2-D colour or
//!    depth target size of the title's 16:9 family is logged once as it is
//!    made (`target`), and the verdict says whether a colour target and a
//!    depth target of exactly the verdict's size exist (`confirmed`).
//!
//! What it prints (stdout, lines starting `render size:`):
//! - `target colour|depth WxH (format F, N so far)`: first time a size is seen;
//! - `world frame N: internal WxH (scale S of the output WxH; D draws of T
//!   there, X at the output size) confirmed=yes|no top=[...]` at world frames
//!   1, 2, 5, 10, 30, 100, 300, 1000 and every 3000th; and
//! - `changed: A -> B at world frame N` whenever the frame's internal size
//!   differs from the last frame's (dynamic resolution, or a pass change;
//!   at most 200 lines).
//!
//! Not verified by a run yet. Its first use is the calibration the plan in
//! `docs/mac/fidelity-experiment.md` asks for: at output 1280x720 (the perf
//! profile) the verdict must be 853x480 (2/3), the figure known from
//! the earlier runs.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::sync::{LazyLock, Mutex, MutexGuard};

const WANTED: &str = "VICTORY_LOG_RENDER_SIZE";

static ENABLED: LazyLock<bool> = LazyLock::new(|| {
    std::env::var(WANTED).ok().is_some_and(|v| !matches!(v.trim(), "" | "0"))
});

/// Whether the diagnostic is on.
pub fn enabled() -> bool {
    *ENABLED
}

type Size = (u32, u32);

#[derive(Default)]
struct Book {
    /// Draws of the frame in progress, by viewport size.
    draws: BTreeMap<Size, u64>,
    /// Targets seen: (size, depth?) -> times made.
    targets: BTreeMap<(Size, bool), u64>,
    /// The size of the picture the host presents (the title's output).
    output: Size,
    /// The previous frame's verdict.
    last: Option<Size>,
    changes: u32,
}

static BOOK: LazyLock<Mutex<Book>> = LazyLock::new(|| Mutex::new(Book::default()));
static SEEN: AtomicU64 = AtomicU64::new(0);
static LOGGED_TARGETS: AtomicBool = AtomicBool::new(false);

fn book() -> MutexGuard<'static, Book> {
    BOOK.lock().unwrap_or_else(|e| e.into_inner())
}

thread_local! {
    /// The viewport last set on this thread: draws are recorded where it was set.
    static VIEWPORT: Cell<Size> = const { Cell::new((0, 0)) };
}

/// The title set viewports: the first one's size, rounded.
pub fn viewport(width: f32, height: f32) {
    if !enabled() {
        return;
    }
    VIEWPORT.with(|v| v.set((width.round().max(0.0) as u32, height.round().max(0.0) as u32)));
}

/// The title recorded a draw (called from `world::draw`).
pub(crate) fn draw() {
    if !enabled() {
        return;
    }
    let size = VIEWPORT.with(Cell::get);
    if size.0 == 0 || size.1 == 0 {
        return;
    }
    *book().draws.entry(size).or_insert(0) += 1;
}

/// The host presents a picture of this size (the output the title runs at);
/// called from the Windows-only host.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn presented(width: u32, height: u32) {
    if !enabled() {
        return;
    }
    book().output = (width, height);
}

/// The title made a texture (any kind: only 2-D targets count).
pub(crate) fn resource(dimension_2d: bool, width: u64, height: u32, array: u16, samples: u32, flags: u32) {
    const RENDER_TARGET: u32 = 1;
    const DEPTH: u32 = 2;
    if !enabled() || !dimension_2d || array != 1 || flags & (RENDER_TARGET | DEPTH) == 0 {
        return;
    }
    let Ok(width) = u32::try_from(width) else { return };
    if width < 320 || height < 180 {
        return;
    }
    let depth = flags & DEPTH != 0;
    let mut book = book();
    let n = book.targets.entry(((width, height), depth)).or_insert(0);
    *n += 1;
    if *n == 1 && sixteen_nine(width, height) {
        let seen = SEEN.fetch_add(1, Relaxed) + 1;
        if seen <= 300 {
            println!(
                "render size: target {} {width}x{height} (samples {samples}, {seen} sizes so far)",
                if depth { "depth" } else { "colour" }
            );
        } else if !LOGGED_TARGETS.swap(true, Relaxed) {
            println!("render size: target lines stop after 300 sizes");
        }
    }
}

fn aspect_matches(size: Size, output: Size) -> bool {
    let (w, h) = (f64::from(size.0), f64::from(size.1));
    let (ow, oh) = (f64::from(output.0), f64::from(output.1));
    h > 0.0 && oh > 0.0 && (w / h / (ow / oh) - 1.0).abs() < 0.01
}

fn sixteen_nine(width: u32, height: u32) -> bool {
    aspect_matches((width, height), (16, 9))
}

fn area(size: Size) -> u64 {
    u64::from(size.0) * u64::from(size.1)
}

/// What the viewport counts of one frame say.
#[derive(Debug, PartialEq)]
pub struct Verdict {
    /// The internal size (the output's when no smaller one has draws).
    pub internal: Size,
    /// Draws at that size, and in all candidates smaller than the output.
    pub draws: u64,
    pub candidate_draws: u64,
    /// Draws at the output size.
    pub at_output: u64,
    /// The candidates, most draws first.
    pub top: Vec<(Size, u64)>,
}

/// The internal size of a frame from its draws per viewport, for the output
/// size `output` (module comment for the rule). `None` without draws at all
/// of the output's aspect.
pub fn verdict(draws: &BTreeMap<Size, u64>, output: Size) -> Option<Verdict> {
    // Without a known output, take 16:9 and no size limit.
    let known = output.0 > 0 && output.1 > 0;
    let reference = if known { output } else { (16, 9) };
    let at_output = if known { draws.get(&output).copied().unwrap_or(0) } else { 0 };
    let mut candidates: Vec<(Size, u64)> = draws
        .iter()
        .filter(|&(&s, _)| aspect_matches(s, reference) && (!known || area(s) < area(output)))
        .map(|(&s, &n)| (s, n))
        .collect();
    // Most draws first; of equal counts, the larger size.
    candidates.sort_by(|a, b| b.1.cmp(&a.1).then(area(b.0).cmp(&area(a.0))));
    let candidate_draws = candidates.iter().map(|c| c.1).sum();
    let top: Vec<_> = candidates.iter().take(5).copied().collect();
    match candidates.first() {
        Some(&(internal, n)) => Some(Verdict { internal, draws: n, candidate_draws, at_output, top }),
        None if at_output > 0 => Some(Verdict {
            internal: output,
            draws: at_output,
            candidate_draws: 0,
            at_output,
            top: Vec::new(),
        }),
        None => None,
    }
}

/// Whether this world frame is one the verdict is printed for.
fn report_frame(n: u64) -> bool {
    matches!(n, 1 | 2 | 5 | 10 | 30 | 100 | 300) || n % 1000 == 0 && n > 0 && (n == 1000 || n % 3000 == 0)
}

/// The title presents; `world_frame` is the world clock after this present.
pub(crate) fn on_present(world_frame: u64) {
    if !enabled() {
        return;
    }
    let mut book = book();
    let draws = std::mem::take(&mut book.draws);
    if world_frame == 0 {
        return;
    }
    let output = book.output;
    let Some(v) = verdict(&draws, output) else { return };
    let frame_name = format!("world frame {world_frame}");
    if let Some(last) = book.last
        && last != v.internal
        && book.changes < 200
    {
        book.changes += 1;
        println!(
            "render size: changed: {}x{} -> {}x{} at {frame_name}",
            last.0, last.1, v.internal.0, v.internal.1
        );
    }
    book.last = Some(v.internal);
    if report_frame(world_frame) {
        let confirmed = [false, true].iter().all(|&depth| book.targets.contains_key(&(v.internal, depth)));
        let scale = if output.0 > 0 {
            format!("{:.4} of the output {}x{}", f64::from(v.internal.0) / f64::from(output.0), output.0, output.1)
        } else {
            "of an output not seen yet".to_string()
        };
        let top: Vec<String> = v.top.iter().map(|((w, h), n)| format!("{w}x{h}:{n}")).collect();
        println!(
            "render size: {frame_name}: internal {}x{} (scale {scale}; {} draws of {} below the output size, {} at the output size) confirmed={} top=[{}]",
            v.internal.0,
            v.internal.1,
            v.draws,
            v.candidate_draws,
            v.at_output,
            if confirmed { "yes" } else { "no" },
            top.join(" ")
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(counts: &[(Size, u64)]) -> BTreeMap<Size, u64> {
        counts.iter().copied().collect()
    }

    /// The known case: output 1280x720 at 2/3 is 853x480.
    #[test]
    fn the_viewport_with_most_draws_below_the_output_is_the_internal_size() {
        let draws = frame(&[
            ((853, 480), 2400),
            ((427, 240), 30),       // a half-size pass
            ((1280, 720), 900),     // the upscaler's result, post, the interface
            ((4096, 4096), 3000),   // a shadow map: square, drops out
            ((1024, 1024), 500),
        ]);
        let v = verdict(&draws, (1280, 720)).unwrap();
        assert_eq!(v.internal, (853, 480));
        assert_eq!(v.draws, 2400);
        assert_eq!(v.candidate_draws, 2430);
        assert_eq!(v.at_output, 900);
        assert_eq!(v.top[0].0, (853, 480));
    }

    #[test]
    fn at_1440p_the_scale_of_two_thirds_gives_1707_by_960() {
        let draws = frame(&[((1707, 960), 5000), ((2560, 1440), 800), ((853, 480), 100)]);
        let v = verdict(&draws, (2560, 1440)).unwrap();
        assert_eq!(v.internal, (1707, 960));
    }

    #[test]
    fn with_nothing_below_the_output_the_internal_size_is_the_output() {
        let draws = frame(&[((2560, 1440), 800), ((2048, 2048), 300)]);
        let v = verdict(&draws, (2560, 1440)).unwrap();
        assert_eq!(v.internal, (2560, 1440));
        assert_eq!(v.candidate_draws, 0);
        assert!(verdict(&frame(&[((2048, 2048), 300)]), (2560, 1440)).is_none());
    }

    #[test]
    fn a_tie_goes_to_the_larger_size_and_an_unknown_output_means_sixteen_nine() {
        let draws = frame(&[((853, 480), 100), ((1280, 720), 100)]);
        assert_eq!(verdict(&draws, (3840, 2160)).unwrap().internal, (1280, 720));
        assert_eq!(verdict(&draws, (0, 0)).unwrap().internal, (1280, 720));
    }

    #[test]
    fn the_report_frames() {
        let hits: Vec<u64> = (1..=9001).filter(|&n| report_frame(n)).collect();
        assert_eq!(hits, [1, 2, 5, 10, 30, 100, 300, 1000, 3000, 6000, 9000]);
    }
}
