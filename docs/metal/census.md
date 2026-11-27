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
```

`translate` is native arm64 (`victory translate` itself is a Windows-only stub on the Mac). Host settings it
tells the translator: wave 32, view limit 2^21 - 1, raw views 2^21 (`victory-gpu` `descriptor/host.rs`).
`convert` options: `--limit N`, `--every N` (sample), `--jobs N` (1 for timings that do not compete),
`--tag T` (row file suffix), `--blended` (16-bit float targets, blending, depth, 4x MSAA), `--compat bounds`
(a different converter output: pipelines no earlier run built), `--lookup FILE` (compile through an archive),
`--archive`, `--archive-dir`, `--archive-name`.

What each kind builds: `cs` the compute pipeline; `vs` and `ms` a render pipeline with a one-constant pixel
shader; `ps` a render pipeline with a full-screen-triangle vertex shader DXC makes from the pixel shader's
input signature (DXC front end of `tools/macos`, x86_64 under Rosetta); `gs` the translator's rectangle-list
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
  1.5 GB in all; open takes 23 to 285 ms each (by size). About 300 KB per compute pipeline, less for the
  graphics ones (the pixel and vertex stages are shared between pipelines).
* Cold compile against archive, serial, 801 pipelines of all four kinds that no earlier run had built
  (`--compat bounds`, every 25th shader): cold 72 s in total (median 26 ms, p95 318 ms, max 1.7 s), the same
  pipelines again without an archive 100 s (the system cache did not help), from the archive 0.28 s in total
  (median 0.24 ms, max 11 ms): about 250 times faster.
* Pipelines built before by an identical metallib are served by Metal's own cache at about 0.3 ms median; the
  archive does not need it.
