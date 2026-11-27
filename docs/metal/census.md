# Shader census and the pipeline archive

`metal/census` (`victory-census`) takes every shader program of the game package through the chain the Mac
backend runs: Xbox ISA, DXIL (`victory_shader::emit::translate`), Metal Shader Converter 4.0.3, `MTL4Compiler`.
It reads the game directory and writes nothing there; everything derived from the game (translated DXIL,
per-shader rows, archives) goes outside the repository, by default under
`~/Documents/GTA6Project/victory-data/census/` (`--out`) and `.../victory-data/metal-archive/`.
Reports hold hashes and error text only. A container's hash is the first 16 hex digits of its BLAKE3, the
hash the game log prints.

## Commands

```sh
. tools/metal/env.sh; export RUSTUP_TOOLCHAIN=1.98.1
cd metal && cargo build --release -p victory-metal-census
victory-census translate                 # every container -> translate.jsonl, vsh/<hash>.vsh (VSH1)
victory-census convert --kind all        # cs vs ps ms gs -> convert-<kind>.jsonl
victory-census convert --kind cs,vs,ps,ms --archive      # same, and an archive of every pipeline built
victory-census report                    # census.json, mac-side.md/.tsv, shared-translator.md
victory-census convert --kind cs --isolate  # one child process per run; an abort or a hang (--timeout S, default 300) is a row, not the end
victory-census denylist                  # msc-denylist.txt from the abort / timeout rows (written by --isolate too)
victory-census opcodes                   # GFX10 instruction counts -> opcodes.jsonl, research/census/opcodes.{json,md}
```

`translate` is native arm64 (`victory translate` itself is a Windows-only stub on the Mac). Host settings it
tells the translator: wave 32, view limit 2^21 - 1, raw views 2^21 (`victory-gpu` `descriptor/host.rs`).
`convert` options: `--limit N`, `--every N` (sample), `--jobs N` (1 for timings that do not compete),
`--tag T` (row file suffix), `--blended` (16-bit float targets, blending, depth, 4x MSAA), `--compat bounds`
(a different converter output: pipelines no earlier run built), `--lookup FILE` (compile through an archive),
`--archive`, `--archive-dir`, `--archive-name`, `--only LIST|FILE` (only these container hashes: a comma list, or a
file with the hash first on each line; a failure sample or a program family can be rebuilt alone. With `--isolate`
the row file is overwritten: always give a new `--tag`).

`--isolate` runs the conversion in a child (one thread) and starts it again after the shader it died on: an
`MscFatal` row when the child's stderr has Metal Shader Converter's `Fatal` line (`SIGABRT`), `ProcessAbort` for
another abort (a Metal assertion), `ProcessTimeout` when the child stays on one shader longer than `--timeout`
seconds (killed). When it ends it writes the **MSC denylist** `<out>/msc-denylist.txt` (`--denylist PATH`):
one line per cs/vs/ps/ms shader of those rows, `<FNV-1a of the DXIL, 16 hex> TAB <census hash> TAB <kind> TAB <class>`,
the file `VICTORY_METAL_MSC_DENYLIST` names for the back (`msc/guard.rs`, docs/metal/msc.md); rewritten after a run
over everything, merged into after a subset (`--only`, `--limit`, `--every`, `--tag`; the tagged rows count).
Geometry-shader rows are not listed: their input is a vertex shader that converts on its own. `convert` ignores
`VICTORY_METAL_MSC_DENYLIST` (a census must see the aborts). A shader the guard refuses (double precision) is a
`Refused:Doubles` row (`backend`), not an abort.

`opcodes` does not translate: it decodes the native programs of every container (`native_programs`, `gfx10::decode`,
read only) and counts instructions by AMD mnemonic and by class (GDS, BVH, mode registers, 64-bit atomics, lane
operations, DPP, barriers, LDS, interpolation, f64, scalar hints; the class tests are in `census/src/opcodes.rs`),
in programs, distinct code and instructions, plus the `s_setreg` targets and the LDS sizes. About 10 s for the
package. `opcodes.jsonl` (per container, game-derived) goes to `--out`; `opcodes.json` and `opcodes.md` (hashes and
counts only) to `--research` (default `research/census`, outside the repository).

What each kind builds: `cs` the compute pipeline; `vs` and `ms` a render pipeline with a one-constant pixel
shader; `ps` a render pipeline with a full-screen-triangle vertex shader DXC makes from the pixel shader's
input signature (DXC front end of `tools/macos`, x86_64 under Rosetta); `gs` the translator's rectangle-list
geometry shader (`rect_list_gs`) of each vertex shader through MSC and library load. A pipeline the back refuses
(`Unsupported`, for example the 29 vertex shaders with `SV_ClipDistance`, which Metal cannot make a geometry-emulation
mesh function for) is a failed row, not an abort. The title's own geometry

`--archive`, `--archive-dir`, `--archive-name`, `--only FILE` (just the hashes listed in FILE, one per line).
`VICTORY_ATTRIBUTE_AT_VERTEX=0` makes `translate` emit the Metal form of `GetAttributeAtVertex` (mac S1,
docs/metal/attribute-at-vertex.md; the default is the Windows form, which MSC refuses).

What each kind builds: `cs` the compute pipeline; `vs` and `ms` a render pipeline with a one-constant pixel
shader; `ps` a render pipeline with a full-screen-triangle vertex shader DXC makes from the pixel shader's
input signature (DXC front end of `tools/macos`, x86_64 under Rosetta; DXC refuses and aborts on a vertex shader
with more than 31 non-system outputs, so such a pixel shader gets a one-triangle mesh shader (`ms_6_6`, unvalidated)
instead, the row says `"partner":"ms"`); `gs` the translator's rectangle-list
geometry shader (`rect_list_gs`) of each vertex shader through MSC and library load. The title's own geometry
programs have no host shader (the translator refuses them), so there is nothing to convert.

## The pipeline archive

Source: SDK 27.2 headers (`Metal.framework/Headers`), bound by `objc2-metal` 0.3.2; code in
`metal/back/src/archive.rs`.

* `MTLDevice.newPipelineDataSetSerializerWithDescriptor:` makes a serializer; handed to the compiler through
  `MTL4CompilerDescriptor.pipelineDataSetSerializer` (so it must exist when the compiler is made) it collects
  every pipeline that compiler creates. `serializeAsArchiveAndFlushToURL:error:` writes the archive.
  **Use `configuration = CaptureBinaries` alone.** With `CaptureDescriptors | CaptureBinaries` (or descriptors
  alone) the flush returns NO with a nil error on this machine (macOS 27, M5 Max).
* `MTLDevice.newArchiveWithURL:error:` opens it; `MTL4CompilerTaskOptions.lookupArchives` is the lookup: the
  compiler takes a matching entry from the archive instead of compiling.

In the backend: `archive::arm_capture()` before `Device::new`, compile, `archive::flush(path)`. On a later run:
`archive::open(&device, path)` then `archive::set_lookup(&[&archive, ...])` (one or several archives). Every
pipeline compile of the backend (compute, render, mesh) passes the task options, so that one call is the whole
wiring: the host's pipeline creation needs no other change. Wiring it into `victory-gpu`'s pipeline path (where
the archive file lives, when it is opened) is left to the integration.

Measured on an Apple M5 Max, macOS 27, SDK 27.2 (2026-11-27):

* One archive for all 20 391 pipelines fails at the flush: `MDB_MAP_FULL: Environment mapsize limit reached`
  (the archive is an LMDB database). Per kind it works, so the warm-up set is four archives:
  compute 796 MB (2 732 pipelines), vertex 51 MB (2 192), pixel 297 MB (7 214), mesh 360 MB (8 245), about
  1.5 GB in all; open takes 11 to 100 ms each (by size). Flushing takes 2 to 10 s.
* Cold compile against archive, serial, 801 pipelines of all four kinds that no earlier run had built
  (`--compat bounds`, every 25th shader): cold (with the serializer capturing) 158 s in total (median 65 ms,
  p95 869 ms, max 4.5 s), the same pipelines again without an archive 143 s (median 24 ms: Metal's own cache
  helps a little, the long tail is compiled again), from the archive 0.30 s in total (median 0.31 ms, p95
  0.65 ms, max 16 ms): about 500 times faster in total.
* The full archives answer every pipeline of the census in 0.4 to 3 s per kind (serial), median 0.15 to 0.31 ms,
  against 3 671 s (compute), 871 s (mesh), 739 s (pixel), 52 s (vertex) of pipeline compile time cold on 18 threads.
