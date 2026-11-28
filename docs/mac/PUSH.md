# Checking before a push to the public fork (`tools/mac/push-check.sh`)

`fork` (PigBun-AI/VIctory-macOS) is public. Run this before every push of `mac/livetr` (or any
branch) to it:

```sh
tools/mac/push-check.sh                # the checked-out branch (HEAD)
tools/mac/push-check.sh mac/livetr     # a named branch
```

The range checked is `git rev-list <branch> --not --remotes=fork`: all commits the fork does not
have. `git fetch fork` first so the range is not larger than it should be. Exit code 0 = clean,
1 = something blocks the push, 2 = the check could not run (for example no word list).

## What it checks

| | check | result |
|---|---|---|
| (a) | `cargo xtask policy` (on the checked-out tree; uses `tools/mac/env.sh` when present) | BLOCK on failure |
| (b) | credentials in added lines (and in commit messages): literal tokens (`ghp_`, `github_pat_`, `AKIA...`, `xox*-`...), private-key headers, quoted literal values assigned to PASSWORD/TOKEN/SECRET/API_KEY | BLOCK |
| (b) | any other line with PASSWORD / TOKEN / SECRET / askpass / API_KEY in it | REVIEW: printed for a human to judge (most are variable names), does not change the exit code |
| (c) | RFC 1918 addresses (10/8, 172.16/12, 192.168/16); user-home paths under `/Users/<name>/` and `C:\Users\<name>` (generic names such as Shared or placeholders such as `<name>` pass); Windows drive project paths (a drive letter followed by a folder such as project, GTA6, work, src, Documents); `*.local` e-mail addresses; words from the private word list (in file contents, file names and commit messages) | BLOCK |
| (c) | `mac:` commits whose author or committer is not `PigBun-AI <261591371+PigBun-AI@users.noreply.github.com>` (Windows-line commits are not checked for authorship) | BLOCK |
| (d) | added/changed binary files, blobs over 1 MB, game-file extensions (`.rpf`, `.ytd`, `.xvd`, `.bik`, `.cdxil`, ...) | BLOCK |
| (e) | class **nda-gdkx** (`push-check-nda.py`, run first and by `--nda-only`; the pre-push hook runs it): NDA SDK header names (`d3d12_x*.h`, `xg*.h`, `XGame*.h`, `*_xs.h`, ...), `gdkx`/`gxdk` directories, `private/` path segments | BLOCK always: no REVIEW, no waiver, no flag turns it off; see `docs/mac/RUN.md` "GDKX (NDA)" |

Findings are printed as `<commit> <file>:<line>`, and the line with the matched text replaced by
`<match>`, `<private-word>` or `<redacted>`: the private word or the secret is never printed, open
the file at that line. Findings in commits that came from the Windows line (a sync merge brings
them in) are reported the same way; they are the Windows team's to fix, so decide by hand, or run
with `--windows-review` to downgrade findings in non-`mac:` commits to REVIEW.

Options: `--skip-policy`, `--remote NAME` (default `fork`), `--words FILE`, `--no-words` (skip the
word list, for a machine that has none; not for a real push), `--windows-review`, `--strict`
(REVIEW items block too), `--help`.

## The private word list

Real names, user names, host names and the like must **not** be written in the repository, so they
are not in this script. The script reads them, one per line (`#` starts a comment, `w:` in front
means whole word only, matching is case-insensitive), from the first of:

1. `tools/mac/privacy.local.txt` in the checkout (untracked, listed in `.gitignore`),
2. the file named by `PRIVACY_WORDS_FILE` (or `--words FILE`),
3. `privacy.local.txt` in the repository's common git directory (`git rev-parse --git-common-dir`),
   which every worktree of the repository shares.

Values of the environment variables `PIGBUN_CRED_*_USER`, `_USERNAME` and `_HOST` (for example under
`credential_exec`) are added when present. If no word list can be found the check stops with exit 2.

Never commit the word list; if `git status` shows it, the `.gitignore` entry is missing.
