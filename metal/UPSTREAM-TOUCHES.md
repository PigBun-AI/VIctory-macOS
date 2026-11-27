# Upstream touches

Every place where `metal-backend` changes a file that belongs to upstream
(base: the local branch `mac/livetr`, the Mac integration line; its `mac:` patch layer belongs to the Mac integration lead and is not counted here). Layering rules 1 and 10 and
architecture note 3.3: only the hooks on the allow-list in
`tools/metal/check-touches.sh` may appear, each at most 5 changed lines, each
with a row below; `tools/metal/check-touches.sh` fails otherwise. Check it
before and after every rebase.

**当前 0 处** (current: 0). M1 expects to stay at 0.

| file:line | why | how to take it back |
|---|---|---|

Row format (one row per place; the file path must be on the allow-list):

    | `crates/victory-gpu/src/lib.rs:42` | one `mod in_place;` line for M4 | delete when upstream merges #3102 |

Allow-list (architecture note 3.3 and 3.4): `crates/victory-gpu/src/backend.rs`,
`crates/victory-gpu/src/lib.rs`, `crates/victory-gpu/src/shadow.rs`,
`crates/victory-gpu/src/host.rs`, `tools/macos/victory.sh`. New files under
`metal/`, `docs/metal/`, `tools/metal/`, `tests/metal/` are not touches.
