//! Mac patch I1 (docs/mac/PATCHES.md): deterministic input for repeatable
//! benchmark runs and frame captures. Ported from VIctory-macOS
//! (`gameinput_script.rs`, #423), re-done on this line's input path.
//!
//! Everything is off unless a variable is set; with none set the hub reads
//! the host exactly as before.
//!
//! * `VICTORY_INPUT_RECORD=<file.jsonl>`: writes every change of a pad or of
//!   the held keys, tagged with the world frame
//!   (`victory_gpu::mac::world::world_frame`), one JSON object per line:
//!   `{"wframe":120,"ms":3041,"slot":0,"connected":true,"buttons":4096,"lt":0.0,
//!   "rt":0.0,"lx":0.0,"ly":1.0,"rx":0.0,"ry":0.0}` for a pad
//!   (`"connected":false` when it went away) and
//!   `{"wframe":120,"ms":3041,"keys":[{"vk":87,"scan":17,"cp":0,"dead":false}]}`
//!   for the keyboard. Nothing is written while the world frame is 0, and the
//!   file may not lie inside a source tree (`.victory-root`).
//! * `VICTORY_INPUT_REPLAY=<file.jsonl>`: feeds the title from such a file
//!   instead of the host's pads and keyboard. Besides recorded lines it takes
//!   range lines, `{"from":0,"to":120,"ly":1.0,"rt":0.8,"buttons":4,"keys":[87]}`
//!   (pad `slot` 0 by default; fields not named keep their rest value; `keys`
//!   are virtual-key numbers, one-character strings or `VICTORY_KEYS` names
//!   such as `"Enter"`), and `#` or `//` comment lines. A reading holds until
//!   the next recorded line; ranges are added on top for the world frames
//!   `from..=to`. `{"from":0,"to":100000000}` is the neutral input: no stick,
//!   button or key for the whole run.
//!
//! Timing: a replay is indexed by world frame, so it does not depend on how
//! long the menus take or on the frame rate. While the world frame is 0
//! (menus, loading) the pad stays neutral (slot 0 connected, so the title does
//! not show a disconnect prompt) and the keyboard passes through, including
//! `VICTORY_KEYS`, so scripted keys can still get the title past its prompts.
//! From world frame 1 the replay is exclusive: host pads, host keys and
//! `VICTORY_KEYS` no longer reach the title, and rumble is not sent to a host
//! pad. Recording together with replaying records what the title was given.

mod json;
pub(super) mod keys;

use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::Path;
use std::time::Instant;

use json::{obj_bool, obj_f32, obj_keys, obj_u64, parse_json_object, parse_keys_value};

use super::{KeyState, PADS, PadState, Source};

#[derive(Clone, Debug, PartialEq)]
struct TracePadEntry {
    wframe: u64,
    slot: usize,
    connected: bool,
    pad: PadState,
}

#[derive(Clone, Debug, PartialEq)]
struct TraceKeysEntry {
    wframe: u64,
    keys: Vec<KeyState>,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct RangeEntry {
    from: u64,
    to: u64,
    slot: usize,
    buttons: Option<u32>,
    lt: Option<f32>,
    rt: Option<f32>,
    lx: Option<f32>,
    ly: Option<f32>,
    rx: Option<f32>,
    ry: Option<f32>,
    keys: Vec<KeyState>,
}

#[derive(Clone, Debug, Default)]
struct Script {
    pad_traces: [Vec<TracePadEntry>; PADS],
    key_traces: Vec<TraceKeysEntry>,
    ranges: Vec<RangeEntry>,
}

impl Script {
    fn parse(text: &str) -> Result<Self, String> {
        let mut script = Self::default();
        for (line_no, raw_line) in text.lines().enumerate() {
            let line = raw_line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with("//") {
                continue;
            }
            let obj =
                parse_json_object(line).map_err(|e| format!("line {}: {e}", line_no + 1))?;
            if obj.contains_key("from") || obj.contains_key("to") {
                let from = obj_u64(&obj, "from")?.unwrap_or(0);
                let to = obj_u64(&obj, "to")?.unwrap_or(u64::MAX);
                if to < from {
                    return Err(format!(
                        "line {}: `to` ({to}) < `from` ({from})",
                        line_no + 1
                    ));
                }
                let slot = obj_u64(&obj, "slot")?.unwrap_or(0) as usize;
                if slot >= PADS {
                    return Err(format!(
                        "line {}: pad slot {slot} out of range",
                        line_no + 1
                    ));
                }
                let keys = obj_keys(&obj, "keys")?;
                script.ranges.push(RangeEntry {
                    from,
                    to,
                    slot,
                    buttons: obj_u64(&obj, "buttons")?.map(|v| v as u32),
                    lt: obj_f32(&obj, "lt")?,
                    rt: obj_f32(&obj, "rt")?,
                    lx: obj_f32(&obj, "lx")?,
                    ly: obj_f32(&obj, "ly")?,
                    rx: obj_f32(&obj, "rx")?,
                    ry: obj_f32(&obj, "ry")?,
                    keys,
                });
                continue;
            }

            let wframe = obj_u64(&obj, "wframe")?
                .ok_or_else(|| format!("line {}: missing `wframe` or `from`/`to`", line_no + 1))?;
            if let Some(keys_val) = obj.get("keys")
                && !obj.contains_key("slot")
                && !obj.contains_key("connected")
                && !obj.contains_key("buttons")
            {
                let keys = parse_keys_value(keys_val)
                    .map_err(|e| format!("line {}: {e}", line_no + 1))?;
                script.key_traces.push(TraceKeysEntry { wframe, keys });
                continue;
            }

            let slot = obj_u64(&obj, "slot")?.unwrap_or(0) as usize;
            if slot >= PADS {
                return Err(format!(
                    "line {}: pad slot {slot} out of range",
                    line_no + 1
                ));
            }
            let connected = obj_bool(&obj, "connected")?.unwrap_or(true);
            let pad = PadState {
                buttons: obj_u64(&obj, "buttons")?.unwrap_or(0) as u32,
                left_trigger: obj_f32(&obj, "lt")?.unwrap_or(0.0),
                right_trigger: obj_f32(&obj, "rt")?.unwrap_or(0.0),
                left_x: obj_f32(&obj, "lx")?.unwrap_or(0.0),
                left_y: obj_f32(&obj, "ly")?.unwrap_or(0.0),
                right_x: obj_f32(&obj, "rx")?.unwrap_or(0.0),
                right_y: obj_f32(&obj, "ry")?.unwrap_or(0.0),
            };
            script.pad_traces[slot].push(TracePadEntry {
                wframe,
                slot,
                connected,
                pad,
            });
            if obj.contains_key("keys") {
                let keys = obj_keys(&obj, "keys")?;
                script.key_traces.push(TraceKeysEntry { wframe, keys });
            }
        }
        for entries in &mut script.pad_traces {
            entries.sort_by_key(|e| e.wframe);
        }
        script.key_traces.sort_by_key(|e| e.wframe);
        Ok(script)
    }

    fn pad_at(&self, wframe: u64, slot: usize) -> Option<PadState> {
        if slot >= PADS {
            return None;
        }
        // Hold neutral during loading screens (`wframe == 0`), keeping slot 0
        // connected so the title does not show a disconnect prompt.
        if wframe == 0 {
            return (slot == 0).then(PadState::default);
        }
        let mut state = match self.pad_traces[slot].partition_point(|e| e.wframe <= wframe) {
            0 => (slot == 0).then(PadState::default),
            idx => {
                let entry = &self.pad_traces[slot][idx - 1];
                entry.connected.then_some(entry.pad)
            }
        };
        for range in &self.ranges {
            if range.slot == slot && range.from <= wframe && wframe <= range.to {
                let pad = state.get_or_insert_with(PadState::default);
                if let Some(b) = range.buttons {
                    pad.buttons |= b;
                }
                if let Some(v) = range.lt {
                    pad.left_trigger = v;
                }
                if let Some(v) = range.rt {
                    pad.right_trigger = v;
                }
                if let Some(v) = range.lx {
                    pad.left_x = v;
                }
                if let Some(v) = range.ly {
                    pad.left_y = v;
                }
                if let Some(v) = range.rx {
                    pad.right_x = v;
                }
                if let Some(v) = range.ry {
                    pad.right_y = v;
                }
            }
        }
        state
    }

    fn keys_at(&self, wframe: u64) -> Vec<KeyState> {
        if wframe == 0 {
            return Vec::new();
        }
        let mut keys = match self.key_traces.partition_point(|e| e.wframe <= wframe) {
            0 => Vec::new(),
            idx => self.key_traces[idx - 1].keys.clone(),
        };
        for range in &self.ranges {
            if range.from <= wframe && wframe <= range.to {
                for &k in &range.keys {
                    if !keys.contains(&k) {
                        keys.push(k);
                    }
                }
            }
        }
        keys
    }
}

struct Recorder {
    writer: BufWriter<File>,
    started: Instant,
    last_pads: [Option<Option<PadState>>; PADS],
    last_keys: Option<Vec<KeyState>>,
}

impl Recorder {
    fn create(path: &Path) -> Result<Self, String> {
        victory_core::policy::ensure_outside_repo(path).map_err(|e| e.to_string())?;
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("cannot create parent dir for {}: {e}", path.display()))?;
        }
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(path)
            .map_err(|e| format!("cannot open {} for input recording: {e}", path.display()))?;
        Ok(Self {
            writer: BufWriter::new(file),
            started: Instant::now(),
            last_pads: [None; PADS],
            last_keys: None,
        })
    }

    fn record_pad(&mut self, wframe: u64, slot: usize, pad: Option<PadState>) {
        if wframe == 0 || slot >= PADS {
            return;
        }
        if self.last_pads[slot].as_ref() == Some(&pad) {
            return;
        }
        self.last_pads[slot] = Some(pad);
        let ms = self.started.elapsed().as_millis() as u64;
        let line = match pad {
            Some(p) => format!(
                "{{\"wframe\":{},\"ms\":{},\"slot\":{},\"connected\":true,\"buttons\":{},\"lt\":{},\"rt\":{},\"lx\":{},\"ly\":{},\"rx\":{},\"ry\":{}}}\n",
                wframe,
                ms,
                slot,
                p.buttons,
                format_f32(p.left_trigger),
                format_f32(p.right_trigger),
                format_f32(p.left_x),
                format_f32(p.left_y),
                format_f32(p.right_x),
                format_f32(p.right_y),
            ),
            None => format!(
                "{{\"wframe\":{wframe},\"ms\":{ms},\"slot\":{slot},\"connected\":false}}\n"
            ),
        };
        let _ = self.writer.write_all(line.as_bytes());
        let _ = self.writer.flush();
    }

    fn record_keys(&mut self, wframe: u64, keys: &[KeyState]) {
        if wframe == 0 {
            return;
        }
        if self.last_keys.as_deref() == Some(keys) {
            return;
        }
        self.last_keys = Some(keys.to_vec());
        let ms = self.started.elapsed().as_millis() as u64;
        let mut line = format!("{{\"wframe\":{wframe},\"ms\":{ms},\"keys\":[");
        for (i, k) in keys.iter().enumerate() {
            if i > 0 {
                line.push(',');
            }
            use std::fmt::Write;
            let _ = write!(
                line,
                "{{\"vk\":{},\"scan\":{},\"cp\":{},\"dead\":{}}}",
                k.virtual_key, k.scan_code, k.code_point, k.dead_key
            );
        }
        line.push_str("]}\n");
        let _ = self.writer.write_all(line.as_bytes());
        let _ = self.writer.flush();
    }
}

fn format_f32(v: f32) -> String {
    if !v.is_finite() {
        return "0.0".to_owned();
    }
    let s = v.to_string();
    if s.contains('.') || s.contains('e') || s.contains('E') {
        s
    } else {
        format!("{s}.0")
    }
}

pub(super) struct ScriptedSource {
    inner: Box<dyn Source>,
    replay: Option<Script>,
    recorder: Option<Recorder>,
    world_frame: fn() -> u64,
}

impl ScriptedSource {
    fn with_fn(
        inner: Box<dyn Source>,
        replay: Option<Script>,
        recorder: Option<Recorder>,
        world_frame: fn() -> u64,
    ) -> Self {
        Self {
            inner,
            replay,
            recorder,
            world_frame,
        }
    }
}

impl Source for ScriptedSource {
    fn pad(&mut self, slot: usize) -> Option<PadState> {
        let wf = (self.world_frame)();
        let pad = match &self.replay {
            Some(script) => script.pad_at(wf, slot),
            None => self.inner.pad(slot),
        };
        if let Some(rec) = &mut self.recorder {
            rec.record_pad(wf, slot, pad);
        }
        pad
    }

    fn keys(&mut self) -> Vec<KeyState> {
        let wf = (self.world_frame)();
        // While `wf == 0` (menus, loading) pass the host's keys through,
        // `VICTORY_KEYS` among them, so they can reach the 3D world; from
        // `wf >= 1` the replay script is exclusive.
        let keys = match &self.replay {
            Some(script) if wf > 0 => script.keys_at(wf),
            _ => self.inner.keys(),
        };
        if let Some(rec) = &mut self.recorder {
            rec.record_keys(wf, &keys);
        }
        keys
    }

    fn rumble(&mut self, slot: usize, low: u16, high: u16) {
        if self.replay.is_none() {
            self.inner.rumble(slot, low, high);
        }
    }
}

/// The path a variable names, if it is set and not blank.
fn path_of(var: &str) -> Option<String> {
    std::env::var_os(var)
        .map(|v| v.to_string_lossy().trim().to_owned())
        .filter(|s| !s.is_empty())
}

/// Wraps `source` with input replay and/or recording when `VICTORY_INPUT_REPLAY`
/// or `VICTORY_INPUT_RECORD` is set; `source` itself otherwise.
pub(super) fn wrap(source: Box<dyn Source>) -> Box<dyn Source> {
    let replay_path = path_of("VICTORY_INPUT_REPLAY");
    let record_path = path_of("VICTORY_INPUT_RECORD");
    if replay_path.is_none() && record_path.is_none() {
        return source;
    }
    victory_gpu::mac::world::log_latch();

    let replay = replay_path.and_then(|path| match std::fs::read(&path) {
        Ok(bytes) => {
            let sha = victory_gpu::mac::world::digest(&bytes);
            match std::str::from_utf8(&bytes)
                .map_err(|e| format!("invalid UTF-8: {e}"))
                .and_then(Script::parse)
            {
                Ok(script) => {
                    tracing::info!(path, blake3 = %sha, "replaying scripted world-frame inputs (VICTORY_INPUT_REPLAY)");
                    Some(script)
                }
                Err(err) => {
                    tracing::error!(path, error = %err, "failed to parse VICTORY_INPUT_REPLAY; the host's input is used");
                    None
                }
            }
        }
        Err(err) => {
            tracing::error!(path, error = %err, "failed to read VICTORY_INPUT_REPLAY; the host's input is used");
            None
        }
    });

    let recorder = record_path.and_then(|path| match Recorder::create(Path::new(&path)) {
        Ok(rec) => {
            tracing::info!(path, "recording world-frame inputs (VICTORY_INPUT_RECORD)");
            Some(rec)
        }
        Err(err) => {
            tracing::error!(path, error = %err, "failed to open VICTORY_INPUT_RECORD");
            None
        }
    });

    if replay.is_none() && recorder.is_none() {
        return source;
    }

    Box::new(ScriptedSource::with_fn(
        source,
        replay,
        recorder,
        victory_gpu::mac::world::world_frame,
    ))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};

    use super::*;

    #[derive(Default)]
    struct MockHost {
        pads: [Option<PadState>; PADS],
        keys: Vec<KeyState>,
    }

    struct MockSource(Arc<Mutex<MockHost>>);

    impl Source for MockSource {
        fn pad(&mut self, slot: usize) -> Option<PadState> {
            self.0.lock().unwrap().pads[slot]
        }
        fn keys(&mut self) -> Vec<KeyState> {
            self.0.lock().unwrap().keys.clone()
        }
        fn rumble(&mut self, _: usize, _: u16, _: u16) {}
    }

    static TEST_WFRAME: AtomicU64 = AtomicU64::new(0);
    fn test_world_frame() -> u64 {
        TEST_WFRAME.load(Ordering::Relaxed)
    }

    #[test]
    fn record_then_replay_roundtrips_pad_state_and_keys_bit_exact() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trace.jsonl");
        let host = Arc::new(Mutex::new(MockHost::default()));

        let rec = Recorder::create(&path).unwrap();
        let mut src = ScriptedSource::with_fn(
            Box::new(MockSource(Arc::clone(&host))),
            None,
            Some(rec),
            test_world_frame,
        );

        let pad_a = PadState {
            buttons: 0x0405,
            left_trigger: 0.125,
            right_trigger: 0.8125,
            left_x: -0.33333334,
            left_y: 0.9999695,
            right_x: 0.25,
            right_y: -0.75,
        };
        let pad_b = PadState {
            buttons: 0,
            left_y: -0.5,
            ..Default::default()
        };
        let key_w = KeyState {
            scan_code: 0x11,
            code_point: b'w' as u32,
            virtual_key: b'W',
            dead_key: false,
        };

        // Loading screen (wframe == 0) is not recorded.
        TEST_WFRAME.store(0, Ordering::Relaxed);
        host.lock().unwrap().pads[0] = Some(pad_a);
        assert_eq!(src.pad(0), Some(pad_a));

        TEST_WFRAME.store(1, Ordering::Relaxed);
        assert_eq!(src.pad(0), Some(pad_a));

        TEST_WFRAME.store(5, Ordering::Relaxed);
        host.lock().unwrap().keys = vec![key_w];
        assert_eq!(src.keys(), vec![key_w]);

        TEST_WFRAME.store(10, Ordering::Relaxed);
        host.lock().unwrap().pads[0] = Some(pad_b);
        host.lock().unwrap().keys.clear();
        assert_eq!(src.pad(0), Some(pad_b));
        assert!(src.keys().is_empty());
        drop(src);

        let raw = std::fs::read_to_string(&path).unwrap();
        let script = Script::parse(&raw).unwrap();

        // Loading screen holds neutral pad 0 and empty script keys.
        assert_eq!(script.pad_at(0, 0), Some(PadState::default()));
        assert!(script.keys_at(0).is_empty());

        let got_1 = script.pad_at(1, 0).unwrap();
        assert_eq!(got_1.buttons, pad_a.buttons);
        assert_eq!(got_1.left_x.to_bits(), pad_a.left_x.to_bits());
        assert_eq!(got_1.left_y.to_bits(), pad_a.left_y.to_bits());
        assert_eq!(got_1.right_trigger.to_bits(), pad_a.right_trigger.to_bits());
        assert_eq!(script.keys_at(5), vec![key_w]);

        let got_10 = script.pad_at(10, 0).unwrap();
        assert_eq!(got_10.buttons, pad_b.buttons);
        assert_eq!(got_10.left_y.to_bits(), pad_b.left_y.to_bits());
        assert!(script.keys_at(10).is_empty());
    }

    #[test]
    fn declarative_range_scripts_wait_on_loading_screen_and_apply_in_world() {
        let text = concat!(
            "# move forward for 120 world frames\n",
            "{\"from\":0,\"to\":120,\"ly\":1.0,\"rt\":0.8}\n",
            "{\"from\":50,\"to\":60,\"buttons\":4,\"keys\":[87]}\n",
        );
        let script = Script::parse(text).unwrap();

        // Loading screen (`wframe == 0`) stays neutral.
        assert_eq!(script.pad_at(0, 0), Some(PadState::default()));
        assert!(script.keys_at(0).is_empty());

        let p1 = script.pad_at(1, 0).unwrap();
        assert_eq!(p1.left_y, 1.0);
        assert_eq!(p1.right_trigger, 0.8);
        assert_eq!(p1.buttons, 0);

        let p55 = script.pad_at(55, 0).unwrap();
        assert_eq!(p55.left_y, 1.0);
        assert_eq!(p55.right_trigger, 0.8);
        assert_eq!(p55.buttons, 4);
        assert_eq!(script.keys_at(55).len(), 1);
        assert_eq!(script.keys_at(55)[0].virtual_key, 87);

        // After `to`, returns to rest.
        assert_eq!(script.pad_at(121, 0), Some(PadState::default()));
        assert!(script.keys_at(121).is_empty());

        // Recording rejects paths inside any `.victory-root` repository tree.
        let inside = Path::new(env!("CARGO_MANIFEST_DIR")).join("input-trace.jsonl");
        assert!(Recorder::create(&inside).is_err());

        // Multi-byte UTF-8 characters inside JSON strings decode without Latin-1 corruption.
        let utf8_script =
            Script::parse("{\"from\":1,\"to\":2,\"note\":\"тест — ✓\",\"ly\":0.5}\n").unwrap();
        assert_eq!(utf8_script.pad_at(1, 0).unwrap().left_y, 0.5);
    }

    /// While `world_frame() == 0` in replay mode, gamepad stays neutral so
    /// loading never drifts, while the underlying `keys()` (which carry
    /// `VICTORY_KEYS`) pass through so menu navigation still drives loading.
    /// Once `world_frame() >= 1`, the replay script takes over exclusively.
    #[test]
    fn replay_passes_through_loading_keys_at_world_frame_zero_then_replays_in_world() {
        static REPLAY_WF: AtomicU64 = AtomicU64::new(0);
        fn replay_wf() -> u64 {
            REPLAY_WF.load(Ordering::Relaxed)
        }

        let enter = KeyState {
            virtual_key: 0x0D,
            ..Default::default()
        };
        let script_key_w = KeyState {
            virtual_key: b'W',
            ..Default::default()
        };
        let host = Arc::new(Mutex::new(MockHost {
            pads: [
                Some(PadState {
                    buttons: 0xFF,
                    left_y: -1.0,
                    ..Default::default()
                }),
                None,
                None,
                None,
            ],
            keys: vec![enter],
        }));

        let script = Script::parse("{\"from\":1,\"to\":10,\"ly\":1.0,\"keys\":[87]}\n").unwrap();
        let mut src = ScriptedSource::with_fn(
            Box::new(MockSource(Arc::clone(&host))),
            Some(script),
            None,
            replay_wf,
        );

        // Menus/loading (`wframe == 0`): pad held neutral, keys pass through.
        REPLAY_WF.store(0, Ordering::Relaxed);
        assert_eq!(src.pad(0), Some(PadState::default()));
        assert_eq!(src.pad(1), None);
        assert_eq!(src.keys(), vec![enter]);

        // World frame 1: replay script drives pad and keys; underlying menu keys suppressed.
        REPLAY_WF.store(1, Ordering::Relaxed);
        let p1 = src.pad(0).unwrap();
        assert_eq!(p1.left_y, 1.0);
        assert_eq!(p1.buttons, 0);
        assert_eq!(src.keys(), vec![script_key_w]);

        // Past range end: replay returns neutral pad and empty keys (still not leaking host keys).
        REPLAY_WF.store(11, Ordering::Relaxed);
        assert_eq!(src.pad(0), Some(PadState::default()));
        assert!(src.keys().is_empty());
    }

    #[test]
    fn variable_fps_replay_produces_identical_inputs_per_world_frame() {
        let script = Script::parse(concat!(
            "{\"from\":1,\"to\":5,\"ly\":1.0}\n",
            "{\"from\":6,\"to\":10,\"rx\":0.5,\"rt\":0.75}\n",
        ))
        .unwrap();

        let simulate = |loading_polls: usize, polls_per_world_frame: usize| {
            let wf = Arc::new(AtomicU64::new(0));
            let mut out = Vec::new();
            for _ in 0..loading_polls {
                out.push((0, script.pad_at(wf.load(Ordering::Relaxed), 0)));
            }
            for frame in 1..=11u64 {
                wf.store(frame, Ordering::Relaxed);
                for _ in 0..polls_per_world_frame {
                    out.push((frame, script.pad_at(frame, 0)));
                }
            }
            out
        };

        let slow_20fps = simulate(15, 1);
        let fast_120fps = simulate(3, 4);
        for frame in 1..=11u64 {
            let a: Vec<_> = slow_20fps
                .iter()
                .filter(|(f, _)| *f == frame)
                .map(|(_, p)| *p)
                .collect();
            let b: Vec<_> = fast_120fps
                .iter()
                .filter(|(f, _)| *f == frame)
                .map(|(_, p)| *p)
                .collect();
            assert!(!a.is_empty() && !b.is_empty());
            assert!(a.iter().all(|p| *p == a[0]));
            assert!(b.iter().all(|p| *p == a[0]));
        }
    }

    /// `tools/mac/input/world-neutral.txt`-style script: nothing pressed, no
    /// stick moved, for the whole run, in every pad slot's absence too.
    #[test]
    fn the_neutral_script_holds_everything_at_rest_for_the_whole_run() {
        let script = Script::parse("{\"from\":0,\"to\":100000000}\n").unwrap();
        for wf in [0, 1, 2, 5000, 100_000_000] {
            assert_eq!(script.pad_at(wf, 0), Some(PadState::default()), "wf {wf}");
            assert_eq!(script.pad_at(wf, 1), None);
            assert!(script.keys_at(wf).is_empty());
        }
        assert_eq!(script.pad_at(100_000_001, 0), Some(PadState::default()));
    }

    #[test]
    fn replay_keys_take_numbers_characters_and_key_names() {
        let script = Script::parse("{\"from\":1,\"to\":9,\"keys\":[87,\"a\",\"Enter\",\"Left\"]}\n").unwrap();
        let vks: Vec<u8> = script.keys_at(3).iter().map(|k| k.virtual_key).collect();
        assert_eq!(vks, [87, b'A', 0x0D, 0x25]);
        assert!(Script::parse("{\"from\":1,\"to\":9,\"keys\":[\"Nope\"]}\n").is_err());
        assert!(Script::parse("{\"from\":9,\"to\":1}\n").is_err());
        assert!(Script::parse("{\"from\":1,\"to\":9,\"slot\":4}\n").is_err());
    }

    /// Recorded in a menu (`wframe` 0) is not written: the file starts at
    /// the world's first frame, so a replay lines up with any menu length.
    #[test]
    fn recording_starts_at_the_first_world_frame() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.jsonl");
        let mut rec = Recorder::create(&path).unwrap();
        rec.record_pad(0, 0, Some(PadState::default()));
        rec.record_keys(0, &[]);
        rec.record_pad(1, 0, Some(PadState::default()));
        rec.record_pad(2, 0, Some(PadState::default())); // unchanged: no line
        rec.record_pad(3, 0, None);
        drop(rec);
        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2, "{text}");
        assert!(lines[0].starts_with("{\"wframe\":1,") && lines[0].contains("\"connected\":true"));
        assert!(lines[1].starts_with("{\"wframe\":3,") && lines[1].contains("\"connected\":false"));
    }
}
