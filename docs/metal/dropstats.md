# Drop statistics (`VICTORY_METAL_DROPSTATS`)

Per frame, how many draws and dispatches the title handed to the Metal backend, how many the backend
did not run, why, and in which pipeline. Off unless the variable names a directory (outside the repository:
the files are derived from the game).

```sh
VICTORY_METAL_DROPSTATS=<dir> tools/metal/run.sh ...      # the run's logs: --out
python3 tools/metal/dropstats-report.py --out summary.md NAME=RUNDIR [NAME=RUNDIR ...]
```

`run.sh` passes the variable on and sets no default. Both halves of the backend (the PE front and the unixlib
back) write into the directory; the front turns the Unix path into a `Z:` path for Wine.

## Files

* `back.csv`: one row per frame; a frame ends at `PRESENT`. Columns are `proto::dropstats::HEADER`: the
  counts of the commands by kind (`draw`, `draw_indexed`, `mesh`, `indirect_draw`, `dispatch`,
  `indirect_dispatch`), `submitted_*` and `dropped_*` (draws include mesh dispatches and indirect draws),
  `noop` (zero-count draws and mesh groups: they run as no-ops, they are not drops), the same by pipeline
  type (`sub_vs/ms/gs/cs/none`, `drop_*`), then `reasons` (`text=n;...`), `pipelines`
  (`hash:TYPE:reason=n;...`) and `extras`. A pipeline's hash is FNV-1a over its DXIL stages' bytes
  (vertex, pixel, amplification, mesh, geometry); the type is VS (vertex + pixel), MS (mesh), GS (a geometry
  stage, the rectangle-list expansion), CS.
* `front.csv`: one row per `ExecuteCommandLists` and per list whose `Close` failed (so rows are not frames);
  `front-present.csv` has the time of every present, and the report puts rows into frames by those times.
  The dxgi and the d3d12 dll of the front are two copies of the code, which is why the files are split this way.
* `back-pso.csv`, `front-pso.csv`: every pipeline creation with time, kind, hash, result, the text of the
  refusal, and in the back the time the Metal compiler call took (`compile_us`).

## What counts as a drop

Back: a command that fails (`refuse` leaves the reason in `State::note`); every later op of the same block
(the first failure stops it: `run_block`); the ops of lists ended but not committed when the block stops; a
block refused before it runs (`block refused ...`); an indirect GS draw skipped on purpose
(`VICTORY_METAL_SKIP_GS_INDIRECT`, which `run.sh` sets to 1: the draw returns `Ok` but is counted).
Front: lists whose `Close` returns an error (the reason carries the source line that set it), and an
`ExecuteCommandLists` the encoder or the link refused.

What the front and the back cannot see: the draws `victory-gpu` drops before it calls D3D12 (no host pipeline,
refused, being made, indirect not run). Those are in the shared layer's own log; with a diagnostics build
(`VICTORY_DIAGNOSTICS=1`, `tools/macos/victory.sh`) it prints per second `dropped N/frame without a host pipeline:
<why>` and `dropstats-report.py` reads them and maps the refused pipelines to ours by the order of the
refusals. That part of the report is an estimate (one-second averages, aligned by time), and says so.

## Archives

`VICTORY_METAL_ARCHIVE=<dir>` opens `cs/ms/ps/vs.mtl4a` from `<dir>` (what `victory-census` wrote, see
`census.md`) when the back is created, and every pipeline compile looks there first. The report estimates the
hit rate from the compiler time (a compile under 5 ms; a found pipeline takes about 0.3 ms).

## Tests

`proto/src/dropstats.rs` (counting, row format, `BlockTracker`), `back/tests/dropstats.rs` (a block through the
interpreter: a failing op, the ops after it, a refused block), `back/src/dropstats.rs` and `front/src/dropstats.rs`
(files, paths, pipelines).
