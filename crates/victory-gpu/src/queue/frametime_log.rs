//! Sample completed host presentations, outside the presenter lock.
use std::{
    fs::{File, OpenOptions},
    io::{self, BufWriter, Write},
    path::Path,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

const LIMIT: usize = 4096;
static LOG: OnceLock<Option<Mutex<Option<Batch<File>>>>> = OnceLock::new();

struct Batch<W: Write> {
    out: BufWriter<W>,
    rows: Vec<(u64, u128, u64)>,
    previous: Option<(Instant, u64)>,
    flushed: Instant,
    limit: usize,
}

impl<W: Write> Batch<W> {
    fn new(out: W, limit: usize, now: Instant) -> Self {
        Self {
            out: BufWriter::new(out),
            rows: Vec::with_capacity(limit),
            previous: None,
            flushed: now,
            limit,
        }
    }

    fn sample(&mut self, frame: u64, now: Instant, drawn: u64) -> io::Result<()> {
        let row = match self.previous.replace((now, drawn)) {
            Some((then, before)) => (frame, (now - then).as_micros(), drawn.wrapping_sub(before)),
            None => (frame, 0, drawn),
        };
        self.rows.push(row);
        if self.rows.len() >= self.limit || now - self.flushed >= Duration::from_secs(1) {
            self.flush()?;
            self.flushed = now;
        }
        Ok(())
    }

    fn flush(&mut self) -> io::Result<()> {
        for (frame, us, draws) in self.rows.drain(..) {
            writeln!(self.out, "{frame},{us},{draws}")?;
        }
        self.out.flush()
    }
}

fn open(path: Option<&Path>) -> io::Result<Option<Batch<File>>> {
    let Some(path) = path else { return Ok(None) };
    let file = OpenOptions::new().append(true).create(true).open(path)?;
    let header = file.metadata()?.len() == 0;
    let mut batch = Batch::new(file, LIMIT, Instant::now());
    if header {
        batch.out.write_all(b"frame,frame_us,draws\n")?;
        batch.out.flush()?;
    }
    Ok(Some(batch))
}

// Static destructors do not run. Register once with the Windows CRT so a
// normal process exit also drains a partial batch from the submission worker.
unsafe extern "C" {
    fn atexit(callback: extern "C" fn()) -> i32;
}
extern "C" fn finish() {
    if let Some(Some(log)) = LOG.get() {
        let batch = log.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(mut batch) = batch
            && let Err(e) = batch.flush()
        {
            tracing::warn!(error = %e, "frametime log final flush failed");
        }
    }
}

fn init() -> io::Result<Option<Mutex<Option<Batch<File>>>>> {
    let path = std::env::var_os("VICTORY_FRAMETIME_LOG");
    let Some(batch) = open(path.as_deref().map(Path::new))? else {
        return Ok(None);
    };
    // SAFETY: `finish` has static lifetime and the CRT callback ABI.
    if unsafe { atexit(finish) } != 0 {
        return Err(io::Error::other(
            "could not register frametime log final flush",
        ));
    }
    Ok(Some(Mutex::new(Some(batch))))
}

pub(super) fn present(frame: u64, shown: bool) {
    let Some(log) = LOG.get_or_init(|| {
        init().unwrap_or_else(|e| {
            tracing::warn!(error = %e, "frametime log disabled");
            None
        })
    }) else {
        return;
    };
    if !shown {
        return;
    }
    let mut slot = log.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(batch) = slot.as_mut()
        && let Err(e) = batch.sample(frame, Instant::now(), crate::draw::drawn())
    {
        *slot = None;
        drop(slot);
        tracing::warn!(error = %e, "frametime log disabled");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn formats_present_delta() {
        let t = Instant::now();
        let mut b = Batch::new(Vec::new(), LIMIT, t);
        b.sample(12, t, 5).unwrap();
        b.sample(14, t + Duration::from_micros(1234), 16).unwrap();
        b.flush().unwrap();
        assert_eq!(b.out.get_ref(), b"12,0,5\n14,1234,11\n");
    }
    #[test]
    fn full_batch_flushes() {
        let t = Instant::now();
        let mut b = Batch::new(Vec::new(), 2, t);
        b.sample(1, t, 0).unwrap();
        assert!(b.out.get_ref().is_empty());
        b.sample(2, t, 0).unwrap();
        assert_eq!(b.out.get_ref(), b"1,0,0\n2,0,0\n");
        assert!(b.rows.is_empty());
    }
    #[test]
    fn disabled_opens_no_file() {
        assert!(open(None).unwrap().is_none());
    }
}
