# Fast iteration: record/replay, shader hot swap, back hot reload

Design note, 2026-11-27. Status: boundaries fixed now; layers 1-2 are built
after the game's first frame, layer 3 after the back is stable.

## The one rule that makes all three cheap

**Everything the front does to the back goes through `Link` (`call` +
`submit`) as proto bytes. No side channels.** Then:

- the stream of sync calls *is* the object creation log (buffers, heaps,
  descriptors, root signatures, pipelines, fences), in order;
- a recording is that stream plus the memory it points at;
- a reloaded back can be rebuilt by replaying the log.

So no code may create back objects by direct Rust calls from `bridge` or
`front` (the bridge only constructs the executor and forwards `Link`), and
anything that needs data from outside (window layer, title memory) gets it
through a sync op whose arguments are recordable.

## Layer 1: record/replay (the main tool)

- Format: the proto dump format (`proto.md`, file header + blocks, with
  `SYNC_RECORD` for sync calls and `BUFFER_DATA` for referenced memory). It
  belongs to proto; versioned like the rest of proto.
- Recorder: in the bridge's Mach-O half (it sees every call and block, and
  runs in the game's address space so it can read the referenced memory).
  Window: from world frame N for M frames (env switch), plus all sync calls
  from process start (the creation log), plus snapshots of every buffer
  range a recorded block reads (title memory, uploads) taken at submit time.
  Written outside the repository only (game-derived data).
- Replayer: `victory-metal-replay` (native arm64, no Wine, no game): loads
  the file, re-creates objects, re-points addresses (old GPU VA -> new),
  replays blocks frame by frame, captures the presented image each frame,
  times each frame (median, p95).
- One command, batch-capable: `tools/metal/replay.sh <recording>...` ->
  JSON (per frame: time, image hash, diff vs reference) + PNG diffs.
- Reference frames: record the same stretch under D3DMetal
  (`VICTORY_CAPTURE`-style host captures of the presented frames), and also
  keep our own last-good replay output as a baseline for regressions.

### No hands in the loop

Every game run is scripted (`tools/metal/run.sh`, defaults since 1134d03):
`VICTORY_KEYS=8-900/3:Enter` walks the intro and the account screen (Enter is
pad A through the keyboard's stand-in pad), and
`VICTORY_INPUT_REPLAY=tools/metal/input/world-neutral.txt` owns the pad from
world frame 1 on. The pair is part of the fixed scene: a recording names the
key spec and input file it was made with (and `input_script_sha256` from the
bench JSON), the recording window is counted in world frames, and the same
pair under D3DMetal gives the reference frames. A scene that needs movement
gets its own input file (declarative ranges by world frame) next to the
neutral one.

### Frame workbench (extends layer 1; same milestone)

Order: after indirect GS draws and a stable menu, together with layers 1-2.

- Capture: one frame's proto stream + referenced memory + the title's own
  shaders and textures, outside the repository (format = proto dump).
- Workbench (native arm64, no game, no Wine): step by draw / pass; view any
  intermediate target (RT, depth, G-buffer, shadow map, UAV); disable a pass
  or draw; hot-swap a shader and replay at once; GPU time per pass
  (Metal 4 counter heaps). First UI: a Metal window with keys, plus an HTML
  report per frame (thumbnails of every pass, timings, diffs) for batch runs.
- Reference: the Windows line (feat/live-translator) has its own frame replay
  (`victory replay`) and per-pass GPU timing on real D3D12. To investigate
  first: whether its capture sits above seam A (console D3D12.X level). If so
  the same capture replays on the Mac through victory-gpu + our back, and the
  two runs can be compared pass by pass with no format bridge; if not, a
  converter from its intermediate images to ours. D3DMetal (same capture
  under Wine) is the secondary reference. The comparison names the first draw
  whose output departs beyond a tolerance.
- Resource viewer: textures (Xbox tiling, BC formats) and vertex streams as
  decoded, to check resource decoding apart from rendering.

## Layer 2: shader hot swap

- The back keys every pipeline by the hash of its inputs (DXIL bytes + root
  signature + state). An override directory (env `VICTORY_METAL_SHADER_DIR`)
  may hold, per hash: replacement DXIL, a metallib, or MSC options.
- A watcher thread notices changes; at the next submit the back rebuilds the
  affected pipelines and swaps them in its handle table (handles are stable,
  so the front never knows). Works in the game and in the replayer.

## Layer 3: back hot reload (later)

- The unixlib becomes a thin, stable shell: it `dlopen`s the real back dylib,
  which exports one C entry point returning `{abi_version, call, submit,
  drain, destroy}` — the `Link` shape plus lifecycle.
- Reload: drain the GPU, keep the creation log, unload, load the new dylib,
  replay the creation log (re-create objects with the same handles), re-upload
  what the log has, accept loss of GPU-private contents (rendered targets are
  rewritten by the next frame). Wrapped title memory lives in the game process
  and is unaffected.
- Needs from today: handles allocated deterministically from the log
  (replaying the same calls gives the same handles), and no back state that
  is not derivable from the log plus memory.
