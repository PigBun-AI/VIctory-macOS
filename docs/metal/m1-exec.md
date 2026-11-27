# M1 exec: the proto interpreter

Written 2026-11-27. Code: `metal/back/src/exec/` (`mod.rs` Executor and link,
`sync.rs` sync calls, `stream.rs` command blocks, `tlab.rs` argument layout,
`handles.rs`). Tests: `metal/back/tests/exec*.rs` (+ `exec_support/`).

`exec` is the one module of `victory-metal-back` that names
`victory-metal-proto` (back therefore depends on proto; `check-layers.sh`
still passes: proto depends on nothing, front does not see back). The other
modules know nothing of the wire protocol.

## Shape

* `Executor::call(op, args, len)` and `Executor::submit(block)` have the
  signatures of `victory_metal_proto::Link`; `Executor` implements `Link`.
  `InProcess` is a `Link` over an `Arc<Executor>` (native tests, the replayer,
  a same-process front). The Wine bridge's Mach-O half calls the same two
  functions.
* `HELLO` makes the `Device`; until then everything but `NOP` and `HELLO` is
  `NOT_INITIALIZED`. `SHUTDOWN` drains the queues (10 s each), drops all
  state, and the executor is back to "no HELLO yet".
* Handles are allocated by the back: one counter per object type, from 1, never
  reused inside a session (a destroyed handle stays dead). 0 and unknown
  handles are `NOT_FOUND`. (After `SHUTDOWN` + `HELLO` the counters start
  again: a handle of the old session may name a new object.)
* Never a panic: bad handle `NOT_FOUND`, bad value `INVALID_ARG` (a buffer
  range past its end, an address in no live buffer, ...), bad framing
  `BAD_STREAM`, bad version `BAD_VERSION`, unknown sync op `UNKNOWN_OP`, things proto
  defines that this back does not do `UNSUPPORTED`. Core `Error` to `Status`:
  `BadArgument`, `NotResident` → `INVALID_ARG`; `PageConflict` → `CONFLICT`;
  `Timeout` → `TIMEOUT`; `DeviceLost(Hung)` → `DEVICE_HUNG`, `(Removed)` →
  `DEVICE_LOST`; `Unsupported`, `NoDevice` → `UNSUPPORTED`; `Metal` →
  `INTERNAL` (`OUT_OF_MEMORY` for buffer/wrap creation).
* Locking: one mutex over the session state. `FENCE_WAIT` (in 250 ms slices) and
  `DESTROY_QUEUE`'s drain do not hold it; a block runs under it.

## Blocks

1. **Validate whole** (no state touched): framing (magic, versions, length, count,
   trailing bytes: the decoder), list nesting, duplicate or null list ids,
   `QUEUE_SIGNAL`/`QUEUE_WAIT` inside a list, list commands outside one, and every
   opcode or variant this back does not implement. A block that fails has **no
   effect** at all. Stream header caps: unknown bit → `BAD_STREAM`; a capability
   the device lacks (e.g. `TRANSLATED` on arm64) → `UNSUPPORTED`.
2. **Run.** A state error (dead handle, value out of range) stops the block at
   that command: the open list and the lists not yet handed to a queue are
   dropped, lists already committed stay, the status is returned.
3. Ended lists are gathered per queue and committed together with one
   `Queue::submit` (one `ExecuteCommandLists`) before a `QUEUE_SIGNAL`/`QUEUE_WAIT`
   on that queue, at the end of the block, and early whenever a queue has
   `max_lists / 2` waiting (the command-buffer pool is finite and `begin_list`
   waits for a free one). **Every commit is followed by `Device::maintain`.**
4. `BEGIN_LIST` clears argument-table slots 0, 1, 2 (heaps, TLAB): slots are
   reused with the addresses of the previous list.
5. Root arguments: each list keeps a shadow of the TLAB (zeros when a root
   signature is set); `SET_ROOT_*` write into it; the first `DISPATCH` after a
   change allocates `layout.size` bytes of the constant ring, copies the shadow
   and binds it at bind point 2. Setting the same root signature again changes
   nothing. A pipeline must belong to the bound root signature.

## Status of every sync op

| Op | State | Notes |
|---|---|---|
| NOP | implemented | always OK |
| HELLO | implemented | `client_proto_version` ≠ 1 → `BAD_VERSION`; `required_caps` the device lacks (or unknown bits) → `UNSUPPORTED`; again after HELLO: idempotent |
| QUERY_CAPS | implemented | strides 24, `max_queues` 16 |
| SHUTDOWN | implemented | |
| CREATE_QUEUE / DESTROY_QUEUE | implemented | kind checked, priority ignored (every queue is a Metal 4 queue); 16 max (`OUT_OF_MEMORY`); destroy drains first (`TIMEOUT` after 10 s keeps the queue) |
| CREATE_BUFFER / DESTROY_BUFFER | implemented | DEFAULT, UPLOAD, READBACK, SHARED; size 0, WRAPPED, unknown kind, flags → `INVALID_ARG`; destroy is deferred while the GPU may use it |
| WRAP_MEMORY | implemented | page aligned, mapped (`msync`: a hole → `INVALID_ARG`), `CONFLICT` if a page is behind another buffer |
| CREATE_DESC_HEAP / DESTROY_DESC_HEAP | implemented for CBV_SRV_UAV and SAMPLER | RTV, DSV → `UNSUPPORTED` (M3); a heap that is not shader visible is a staging heap |
| WRITE_DESCRIPTOR | partly | NONE, CBV, buffer SRV/UAV (RAW, STRUCTURED) implemented. TYPED, `HAS_COUNTER`, SAMPLER → `UNSUPPORTED` |
| COPY_DESCRIPTORS | implemented | same heap kind, memmove semantics |
| CREATE_ROOT_SIGNATURE / DESTROY_ROOT_SIGNATURE | implemented | layout by the rules below |
| CREATE_COMPUTE_PIPELINE | **placeholder**: `UNSUPPORTED` | arguments and the root-signature handle are still checked. Pipelines in tests come from `Executor::create_compute_pipeline_from_msl` (not on the wire) |
| DESTROY_PIPELINE | implemented | |
| CREATE_FENCE / DESTROY_FENCE / FENCE_VALUE / FENCE_WAIT / FENCE_SIGNAL | implemented | signal never lowers; wait forever = `u64::MAX` |
| DEVICE_STATUS | implemented | worst state of any queue |
| GET_SURFACE_LAYER | **placeholder**: `UNSUPPORTED` | M3 |
| 0x40–0x7FFF | reserved: `UNSUPPORTED` | 24–0x3F and 0x8000+: `UNKNOWN_OP` |

## Status of every command

| Opcode | State | Notes |
|---|---|---|
| BEGIN_LIST / END_LIST | implemented | flags must be 0 |
| QUEUE_SIGNAL / QUEUE_WAIT | implemented | only between lists; commit the queue's waiting lists first |
| DEBUG_LABEL | **placeholder**: accepted, dropped | |
| SET_PIPELINE | implemented | |
| SET_ROOT_SIGNATURE / _CONSTANTS / _DESCRIPTOR / _TABLE | implemented for COMPUTE | GRAPHICS bind point → `UNSUPPORTED`. Constants: range inside the parameter. Descriptor: kind = the parameter's, address 0 or inside a live resident buffer. Table: heap kind matches the table's ranges, index < capacity; the slot holds the entry's GPU address |
| SET_DESCRIPTOR_HEAPS | implemented | each heap must be shader visible and of the right kind; null leaves a binding alone |
| DISPATCH | implemented | threads per group come from the pipeline (`numthreads`; the hook takes them) |
| DISPATCH_INDIRECT | **placeholder**: `UNSUPPORTED` | |
| COPY_BUFFER_REGION | implemented | same-buffer overlap → `INVALID_ARG` |
| FILL_BUFFER | partly | only when the four bytes of the value are equal (a blit fill writes a byte); other patterns → `UNSUPPORTED` |
| CLEAR_UAV | partly | raw or `R32_UINT/SINT/FLOAT`, and the same byte-equal rule; other formats → `UNSUPPORTED` |
| BARRIER | MEMORY and ALIASING implemented | LAYOUT → `UNSUPPORTED` (no textures) |
| SYNC_RECORD / BUFFER_DATA | **not executed**: `UNSUPPORTED` | dump replay is the job of a replayer (`tests/exec_replay.rs`), which re-runs the sync calls, remaps handles and addresses, and applies the data |
| reserved ranges (0x0400–0x09FF) | `UNSUPPORTED` | unknown opcodes `BAD_STREAM` |

## TLAB layout (root signature → top-level argument buffer)

Rule used (`exec/tlab.rs`): root parameters in order; CONSTANTS take 4 bytes
per value at 4-byte alignment; root CBV/SRV/UAV (a GPU address) and TABLE (the
GPU address of the table's first heap entry) take 8 bytes at 8-byte
alignment; the total is rounded up to 8; static samplers take no room.
Example: `[CONSTANTS 2, CBV, TABLE]` → constants at 0 and 4, CBV pointer at 8,
table pointer at 16, size 24.

Source: Apple's game-porting-toolkit,
`docs/metal-shader-converter/binding-model.md` (alignment: pointers 8 bytes,
32-bit constants 4 bytes; "MSC adds padding to achieve the correct alignment")
and `reference.md` (a root signature with a table, a CBV and 4 constants),
commit `4f42344`, 2026-06-07.

**To be checked against MSC reflection** (MSC is not installed here): (1) the
guide can be read as "constants first, then pointers" (its offset formula is
"size of the root constants + resource index × 8"); (2) its example with one
static sampler and one table says two entries, the second a sampler table, so
static samplers may take room. `Executor::set_tlab_layout(sig, offsets, size)`
replaces the layout with what reflection reports (validated: alignment,
inside, no overlap); pipelines made before it keep the old layout and
`DISPATCH` refuses the mismatch (`INVALID_ARG`). Tested with a constants-first
layout end to end (`a_reflection_layout_replaces_the_rule_based_one`).

## Known issues

* TLAB layout rules: the two doubts above.
* DXIL pipelines (MSC) are not wired; so `DISPATCH`'s threads per group have no
  wire source yet (reflection's `numthreads`).
* Not done: typed buffer views and counters, sampler objects, textures,
  RTV/DSV, graphics, indirect dispatch, non-byte fill patterns, labels.
* One mutex covers a block. A block that has to wait for the GPU (ring or
  command-buffer pool full, up to 10 s then `TIMEOUT`) waits with it held; a
  `FENCE_SIGNAL` from another thread would queue behind it.
* A state error mid-block leaves the lists committed before it (by a
  `QUEUE_SIGNAL`/`QUEUE_WAIT` or by the early commit) executed.
* Descriptors keep the address they were written with; destroying the buffer
  does not rewrite them (as in the original API). The address is checked at
  write time only.
* `SET_ROOT_TABLE` takes any heap, a staging heap included (only the address
  goes into the TLAB).
* `DESTROY_QUEUE`, `SHUTDOWN` wait 10 s for a queue; `SHUTDOWN` then drops it anyway.
* Queue `kind` and `priority` have no effect.
* Fixed on the way: the Metal 4 compiler aborts the process for a function name that
  is not in the library (`gpu.rs a_missing_entry_point_is_an_error_not_an_abort`);
  `mincore` does not report holes on macOS, so `WRAP_MEMORY` uses `msync`.
