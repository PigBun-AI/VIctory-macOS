//! `victory-shader-exec`: the executor `victory-shader`'s hardware proofs use
//! on macOS (`VICTORY_SHADER_EXEC`). It reads requests from stdin and answers
//! on stdout (`wire`); stderr is for humans. One process serves many
//! requests, so a test run pays for the Metal device and each conversion
//! once. A request that crashes it costs the caller a respawn, not the run.
//! Docs: `docs/metal/shader-exec.md`.

#[cfg(target_vendor = "apple")]
mod req;
#[cfg(target_vendor = "apple")]
mod run;
#[cfg(target_vendor = "apple")]
mod wire;

#[cfg(target_vendor = "apple")]
fn main() {
    use std::io::{stdin, stdout};
    use wire::{R, W, op, status};

    let mut input = stdin().lock();
    let mut output = stdout().lock();
    let mut runner: Option<Result<run::Runner, String>> = None;
    loop {
        let frame = match wire::read_frame(&mut input) {
            Ok(Some(f)) => f,
            Ok(None) => return,
            Err(e) => {
                eprintln!("victory-shader-exec: {e}");
                std::process::exit(2);
            }
        };
        let reply = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            handle(&frame, &mut runner)
        }))
        .unwrap_or_else(|_| {
            let mut w = W::default();
            w.u8(status::FAILED);
            w.str("the executor panicked");
            w.0
        });
        if wire::write_frame(&mut output, &reply).is_err() {
            return;
        }
    }

    fn fail(code: u8, text: &str) -> Vec<u8> {
        let mut w = W::default();
        w.u8(code);
        w.str(text);
        w.0
    }

    fn failure(f: run::Failure) -> Vec<u8> {
        match f {
            run::Failure::Unsupported(t) => fail(status::UNSUPPORTED, &t),
            run::Failure::Failed(t) => fail(status::FAILED, &t),
        }
    }

    fn handle(frame: &[u8], runner: &mut Option<Result<run::Runner, String>>) -> Vec<u8> {
        let mut r = R::new(frame);
        let parsed = (|| -> Result<_, String> {
            let op = r.u8()?;
            Ok(op)
        })();
        let op = match parsed {
            Ok(op) => op,
            Err(e) => return fail(status::FAILED, &e),
        };
        let run = runner.get_or_insert_with(|| {
            run::Runner::new().map_err(|f| match f {
                run::Failure::Unsupported(t) | run::Failure::Failed(t) => t,
            })
        });
        let run = match run {
            Ok(r) => r,
            Err(e) => return fail(status::UNSUPPORTED, e),
        };
        let mut w = W::default();
        match op {
            op::HELLO => {
                let c = run.caps();
                w.u8(status::OK);
                w.u32(wire::VERSION);
                w.str(&c.name);
                w.u32(c.wave_min);
                w.u32(c.wave_max);
                w.u32(u32::from(c.mesh) << 1 | 1);
                w.0
            }
            op::COMPILE | op::DISPATCH => {
                let res = (|| -> Result<(u64, req::Root, &[u8], Option<req::Dispatch>), String> {
                    let root = req::root(&mut r)?;
                    let dxil = r.bytes()?;
                    let key = run::key_of(r.consumed());
                    let d = if op == op::DISPATCH {
                        Some(req::dispatch(&mut r)?)
                    } else {
                        None
                    };
                    r.end()?;
                    Ok((key, root, dxil, d))
                })();
                let (key, root, dxil, d) = match res {
                    Ok(x) => x,
                    Err(e) => return fail(status::FAILED, &format!("bad request: {e}")),
                };
                match d {
                    None => match run.compile(key, &root, dxil) {
                        Ok(()) => {
                            w.u8(status::OK);
                            w.0
                        }
                        Err(f) => failure(f),
                    },
                    Some(d) => match run.dispatch(key, &root, dxil, &d) {
                        Ok(outs) => {
                            w.u8(status::OK);
                            w.u32(outs.len() as u32);
                            for o in outs {
                                w.u32s(&o);
                            }
                            w.0
                        }
                        Err(f) => failure(f),
                    },
                }
            }
            other => fail(status::FAILED, &format!("unknown op {other}")),
        }
    }
}

#[cfg(not(target_vendor = "apple"))]
fn main() {
    eprintln!("victory-shader-exec needs Metal: Apple targets only");
    std::process::exit(2);
}
