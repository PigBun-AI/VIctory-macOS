//! One-shot final-state capture. Sequence numbers in the accompanying ops file
//! are host recording order, NOT a total GPU ordering across command lists.
use std::{fs, io, path::PathBuf, sync::{OnceLock, atomic::{AtomicU64, Ordering}}};
static FRAME: AtomicU64 = AtomicU64::new(u64::MAX);
pub(super) fn dir() -> Option<&'static PathBuf> {
    static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    DIR.get_or_init(|| std::env::var_os("VICTORY_FRAME_DUMP").filter(|s| !s.is_empty()).map(PathBuf::from)).as_ref()
}
pub(super) fn selected(frame: u64) -> bool { FRAME.load(Ordering::Acquire) == frame }
pub(crate) fn boundary(frame: u64) {
    let Some(dir) = dir() else { return };
    let flag = dir.join("trigger.flag");
    if selected(frame) {
        FRAME.store(u64::MAX, Ordering::Release);
        let finish = || -> io::Result<()> {
            let prefix = format!("frame-{frame}-");
            let mut textures = Vec::new();
            for entry in fs::read_dir(dir)? {
                let file = entry?.file_name();
                let Some(file) = file.to_str() else { continue };
                let Some(tail) = file.strip_prefix(&prefix).and_then(|s| s.strip_suffix(".png")) else { continue };
                let Some((_, fmt)) = tail.rsplit_once("-f") else { continue };
                if let Ok(fmt) = fmt.parse::<u32>() && tail.bytes().all(|c| c.is_ascii_hexdigit() || matches!(c, b'-' | b'x')) {
                    textures.push(format!(r#"{{"file":"{file}","dxgi_format":{fmt}}}"#));
                }
            }
            textures.sort();
            fs::write(dir.join("index.json"), format!(r#"{{"frame":{frame},"snapshot":"final state, subresource 0","ops":"frame-{frame}-ops.txt","textures":[{}]}}"#, textures.join(",")))?;
            fs::remove_file(flag)?;
            fs::write(dir.join("done.txt"), format!("frame {frame}; {} textures\n", textures.len()))
        };
        if let Err(error) = finish() { tracing::warn!(%error, "frame dump finalization failed"); }
    } else if flag.is_file() { FRAME.store(frame + 1, Ordering::Release); }
}
