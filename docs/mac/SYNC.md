# Syncing `mac/livetr` with the Windows line (`tools/mac/sync-windows.sh`)

Run it at least once a day. It reads the user's Windows repo (**read only**), picks the most recently
committed development branch, merges it into `mac/livetr` (a merge, never a rebase, so the `mac:`
history stays), runs `tools/mac/check.sh` and writes a report.

```sh
# credentials come from the environment: run it under credential_exec with
# credential-7ed4105d97af436baf552fc3f7863f41 (PIGBUN_CRED_WINDOWS_7ED4_USER / _PASSWORD),
# or export WIN_USER / WIN_PASSWORD yourself. Nothing secret is in the script or written to disk.
# The host, repo path and report directory come from the environment too (see Settings below).
tools/mac/sync-windows.sh --dry-run        # fetch + analyse, predict conflicts, change nothing
tools/mac/sync-windows.sh                  # fetch, merge into mac/livetr, build + test, report
```

Reports go to `$SYNC_DIR/<yyyymmdd-hhmmss>.md` (selected branch, old → new
HEAD, new commits, commits touching Mac patch files, victory-gpu D3D12 / victory-loader / victory-hle
changes, conflicts with the matching `docs/mac/PATCHES.md` entries, build/test results). The state
(`last_branch`, `last_commit`) is the `state` file next to the reports, outside the repo.

## Settings

Nothing site-specific is written in the script: no host name, user name, repo path or absolute
directory. Set these in the environment, or put them in `tools/mac/sync.local.env` (a shell file the
script sources when it exists; it is in `.gitignore`, never commit it):

| variable | meaning |
|---|---|
| `SYNC_WIN_HOST` | the Windows machine to fetch from (needed unless `--no-fetch`) |
| `SYNC_WIN_REPO` | the repo path as git on Windows sees it, e.g. `X:/path/to/repo` (needed unless `--no-fetch`) |
| `SYNC_DIR` | directory for the reports and the `state` file (not needed if `--state` and `--report`/`--report-dir` are both given) |
| `WIN_USER`, `WIN_PASSWORD` | the Windows login; default: `PIGBUN_CRED_WINDOWS_7ED4_USER` / `_PASSWORD` from the credential |

If one is missing the script stops with exit 1 and lists the variables to set. Example
`tools/mac/sync.local.env` (values are yours, not in the repo):

```sh
export SYNC_WIN_HOST=<windows host>
export SYNC_WIN_REPO=<repo path on Windows>
export SYNC_DIR="$HOME/<some folder>/sync"
```

## What it does

1. `git ls-remote --heads` on Windows, then `git fetch --no-tags --prune` of `refs/heads/*` into
   `refs/remotes/win-all/*` (in the VIctory-macOS repo, whose work tree is never touched). Only
   `git upload-pack` runs on Windows. The chosen branch is also kept as `refs/remotes/win/<name with / as ->`.
2. Candidates = those branches minus the excluded ones (default: `main`/`master`, `tmp/ temp/ backup/
   bak/ scratch/ old/ archive/ sync/`, and names containing backup/bak/tmp/temp/old as a word);
   sorted by the tip's committer date, top 5 printed. Rule: `SYNC_EXCLUDE_REGEX` (replaces) or
   `--exclude REGEX` (adds). `--branch NAME` picks one by hand.
3. If the branch differs from the one in the state file, a red banner is printed and the report says so:
   that may need a decision.
4. **Held (exit 3)** when `mac/livetr` has Windows-line commits the new branch lacks (other lineage,
   or Windows rewrote history), when there is no common ancestor, or when the branch is more than
   `SYNC_MAX_AHEAD` (300) commits ahead. The report gives the merge-base and the predicted conflicts;
   `--force-switch` merges anyway. The state file is not updated while held.
5. The work tree must be on the target branch with no uncommitted changes to tracked files (untracked
   files only warn), no merge/rebase/cherry-pick in progress; otherwise it refuses (exit 1).
6. `git merge --no-ff` of the new tip. On conflict: `git merge --abort`, tree back as before, the
   report lists the conflicted files with the `mac:` commits and PATCHES.md entries that touch them
   (exit 2).
7. `tools/mac/check.sh` (native build, native tests, cross build of `victory.exe`; logs in
   `<worktree>/target/mac-logs`). Result: pass/fail per step and the names of failed tests. On failure
   the merge is **kept** and the report is marked 🔴 (exit 4); `--rollback-on-fail` resets the branch to
   the old HEAD instead (only if nothing else moved it).

## Options

| option | meaning |
|---|---|
| `--dry-run` | fetch and analyse only (also a `merge-tree` conflict prediction); no merge, no build, no state change |
| `--branch NAME` | use this Windows branch |
| `--worktree PATH`, `--target-branch NAME` | where to merge (default: the checkout holding the script, branch `mac/livetr`); for tests use a temporary worktree and branch |
| `--force-switch` | merge even if held |
| `--rollback-on-fail` | undo the merge when build/test fails |
| `--no-check`, `--check-anyway` | skip the checks / run them even with nothing new |
| `--push-fork` | after a green run push the target branch to remote `fork` (PigBun-AI/VIctory-macOS), plain push; **off by default**; nothing is ever pushed to Windows or any other upstream |
| `--no-fetch` | do not contact Windows (use the refs already fetched) |
| `--report FILE`, `--report-dir DIR`, `--state FILE`, `--top N`, `--exclude REGEX` | paths and tuning |

Exit codes: 0 ok / up to date / dry run, 1 refused or error, 2 conflict, 3 held, 4 merged but build/test
failed, 5 Windows unreachable. Environment knobs: `SYNC_DIR`, `SYNC_WIN_HOST`, `SYNC_WIN_REPO` (the three required ones, above),
`SYNC_MAX_AHEAD`, `SYNC_EXCLUDE_REGEX`, `SYNC_CHECK_TIMEOUT` (seconds, default 7200).

## Testing without touching a live worktree

```sh
git -C ../VIctory-macOS worktree add ../VIctory-livetr-synctest -b tmp/sync-test mac/livetr
cp -cR target/debug ../VIctory-livetr-synctest/target/debug   # optional: APFS clone, saves the cold build
../VIctory-livetr-synctest/tools/mac/sync-windows.sh --target-branch tmp/sync-test \
    --state /tmp/sync-state --report-dir /tmp/sync-reports [--dry-run]
git -C ../VIctory-macOS worktree remove --force ../VIctory-livetr-synctest && git -C ../VIctory-macOS branch -D tmp/sync-test
```
Conflict / failure scenarios: make a commit on top of the Windows tip, point
`refs/remotes/win-all/synctest/<x>` at it and run with `--no-fetch --branch synctest/<x>`.
