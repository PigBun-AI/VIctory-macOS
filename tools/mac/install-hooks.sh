#!/usr/bin/env bash
# tools/mac/install-hooks.sh -- install the shared pre-push hook that enforces the nda-gdkx class of push-check.
#
#   tools/mac/install-hooks.sh            install / refresh (safe to run again: it never inserts the block twice)
#   tools/mac/install-hooks.sh --check    only report whether the hook is installed and current (exit 1 when not)
#
# The hook goes to <git common dir>/hooks/pre-push, which every worktree of the repository shares (it never enters git). It
# is a managed block between two marker lines, put right after the shebang; whatever pre-push script was there stays below
# it, unchanged, and still gets the ref list on stdin and the original arguments. The block runs the nda-gdkx check
# (tools/mac/push-check-nda.py, the engine of `push-check.sh --nda-only`) over what each ref would add to the remote and
# refuses the push when anything hits. The engine is copied to <git common dir>/gdkx-guard/ so that it holds for every
# worktree and every branch, old or new, and a branch cannot switch it off by editing its own copy: after changing
# tools/mac/push-check-nda.py run this script again. `git push --no-verify` skips every hook; do not use it (docs/mac/RUN.md).
set -eu
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
CHECK_ONLY=0
case "${1:-}" in
    "") ;;
    --check) CHECK_ONLY=1 ;;
    -h|--help) sed -n '2,/^set -eu/p' "$0" | sed '$d; s/^# \{0,1\}//'; exit 0 ;;
    *) echo "install-hooks: unknown option: $1" >&2; exit 2 ;;
esac

COMMON="$(git -C "$ROOT" rev-parse --path-format=absolute --git-common-dir)"
HOOKS="$(git -C "$ROOT" config --get core.hooksPath || true)"
if [ -n "$HOOKS" ]; then
    case "$HOOKS" in /*) ;; *) HOOKS="$ROOT/$HOOKS" ;; esac
else
    HOOKS="$COMMON/hooks"
fi
HOOK="$HOOKS/pre-push"
ENGINE_DIR="$COMMON/gdkx-guard"
ENGINE="$ENGINE_DIR/push-check-nda.py"
BEGIN='# >>> gdkx-guard (managed by tools/mac/install-hooks.sh; do not edit between the markers) >>>'
END='# <<< gdkx-guard <<<'

block() {
    cat <<'BLOCK'
# >>> gdkx-guard (managed by tools/mac/install-hooks.sh; do not edit between the markers) >>>
# nda-gdkx: refuse a push that would put NDA SDK material on a remote. Reads the ref list, then hands it on to what follows.
__gg_in="$(mktemp "${TMPDIR:-/tmp}/pre-push.XXXXXX")" || { echo "gdkx-guard: no temp file; push refused" >&2; exit 1; }
cat > "$__gg_in"
__gg_remote="$1"
__gg_top="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
__gg_common="$(git rev-parse --path-format=absolute --git-common-dir)"
__gg_engine="$__gg_common/gdkx-guard/push-check-nda.py"
__gg_rc=0
if [ ! -f "$__gg_engine" ] || ! command -v python3 >/dev/null 2>&1; then
    echo "gdkx-guard: the nda-gdkx engine ($__gg_engine) or python3 is missing; push refused (run tools/mac/install-hooks.sh)" >&2
    __gg_rc=1
else
    while read -r __gg_lref __gg_lsha __gg_rref __gg_rsha; do
        case "$__gg_lsha" in ""|*[!0-9a-f]*) echo "gdkx-guard: unreadable ref line; push refused" >&2; __gg_rc=1; continue ;; esac
        case "$__gg_lsha" in *[!0]*) ;; *) continue ;; esac   # all zeros = a deletion: nothing to scan
        __gg_not=""
        case "$__gg_rsha" in ""|*[!0-9a-f]*) ;; *[!0]*) if git cat-file -e "$__gg_rsha^{commit}" 2>/dev/null; then __gg_not="^$__gg_rsha"; fi ;; esac
        echo "gdkx-guard: nda-gdkx check of $__gg_lref -> $__gg_remote"
        python3 "$__gg_engine" --root "$__gg_top" range "$__gg_lsha" --not "--remotes=$__gg_remote" ${__gg_not:+"$__gg_not"} || {
            echo "gdkx-guard: PUSH REFUSED for $__gg_lref (class nda-gdkx; see docs/mac/RUN.md, GDKX section)" >&2
            __gg_rc=1
        }
    done < "$__gg_in"
fi
[ "$__gg_rc" -eq 0 ] || { rm -f "$__gg_in"; exit 1; }
exec < "$__gg_in"; rm -f "$__gg_in"
# <<< gdkx-guard <<<
BLOCK
}

current_block() { [ -f "$HOOK" ] && awk -v b="$BEGIN" -v e="$END" '$0==b{p=1} p{print} $0==e{p=0}' "$HOOK" || true; }

if [ "$CHECK_ONLY" = 1 ]; then
    ok=1
    [ -x "$HOOK" ] || { echo "install-hooks: $HOOK missing or not executable"; ok=0; }
    [ "$(current_block)" = "$(block)" ] || { echo "install-hooks: the managed block is missing or out of date"; ok=0; }
    cmp -s "$HERE/push-check-nda.py" "$ENGINE" || { echo "install-hooks: the installed engine is missing or differs from tools/mac/push-check-nda.py"; ok=0; }
    if [ "$ok" = 1 ]; then echo "install-hooks: pre-push hook installed and current ($HOOK)"; exit 0; fi
    exit 1
fi

mkdir -p "$HOOKS" "$ENGINE_DIR"
cp "$HERE/push-check-nda.py" "$ENGINE.tmp" && chmod 755 "$ENGINE.tmp" && mv "$ENGINE.tmp" "$ENGINE"

TMP="$(mktemp "$HOOKS/.pre-push.XXXXXX")"
trap 'rm -f "$TMP"' EXIT
if [ -f "$HOOK" ]; then
    # drop an old managed block, keep every other byte
    awk -v b="$BEGIN" -v e="$END" '$0==b{skip=1} !skip{print} $0==e{skip=0}' "$HOOK" > "$TMP.body"
else
    : > "$TMP.body"
fi
FIRST="$(head -n 1 "$TMP.body" || true)"
case "$FIRST" in
    "#!"*)
        case "$FIRST" in
            *sh|*"sh "*|*"sh") ;;  # sh, bash, dash, zsh, "env bash"
            *) echo "install-hooks: the existing pre-push is not a shell script ($FIRST); not touching it" >&2; rm -f "$TMP.body"; exit 1 ;;
        esac
        { echo "$FIRST"; block; tail -n +2 "$TMP.body"; } > "$TMP" ;;
    "")
        { echo '#!/bin/sh'; block; echo 'exit 0'; } > "$TMP" ;;
    *)
        { echo '#!/bin/sh'; block; cat "$TMP.body"; } > "$TMP" ;;
esac
rm -f "$TMP.body"
chmod 755 "$TMP"
mv "$TMP" "$HOOK"
trap - EXIT
echo "install-hooks: installed $HOOK (engine: $ENGINE)"
"$0" --check
