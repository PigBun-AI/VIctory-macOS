#!/bin/sh
# Rule 1 and 10 (architecture note 3.3, 3.4): the metal-backend branch changes
# nothing upstream except the allow-listed hooks, and every hook is written
# down in metal/UPSTREAM-TOUCHES.md. Run before and after every rebase.
#
# What counts as a change: everything between the merge-base of mac/livetr and
# HEAD (commits; the base is the local branch mac/livetr, the Mac integration
# line = the Windows line's feat/live-translator plus the "mac:" patch layer,
# which belongs to the Mac integration lead and is not ours to count), plus uncommitted edits and untracked files, so it also
# catches work that is not committed yet. Renames are not followed (a
# moved-away upstream file shows up as a deletion).
#
#   allowed freely   metal/  docs/metal/  tools/metal/  tests/metal/
#   hooks (<= 5 changed lines per file, each listed in metal/UPSTREAM-TOUCHES.md)
#                    crates/victory-gpu/src/{backend,lib,shadow,host}.rs
#                    tools/macos/victory.sh
#   anything else    FORBIDDEN
#
# The document states how many places there are ("当前 N 处") and has one table
# row per place (`| \`path:line\` | why | recall |`); N, the rows and the files
# actually touched must agree.
#
#   tools/metal/check-touches.sh               check this checkout
#   tools/metal/check-touches.sh --self-test   prove each rule on a scratch repo
#
# BASE_REF (default mac/livetr) and REPO_DIR / TOUCHES_DOC override where it looks.
set -u
here=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$here/../.." && pwd)

ALLOWED='^(metal/|docs/metal/|tools/metal/|tests/metal/)'
HOOKS='^(crates/victory-gpu/src/(backend|lib|shadow|host)\.rs|tools/macos/victory\.sh)$'
MAX_HOOK_LINES=5

check() {
    repo=${REPO_DIR:-$ROOT}
    doc=${TOUCHES_DOC:-$repo/metal/UPSTREAM-TOUCHES.md}
    base_ref=${BASE_REF:-mac/livetr}
    failed=0
    g() { git -C "$repo" "$@"; }

    if ! g rev-parse --verify --quiet "$base_ref^{commit}" >/dev/null; then
        echo "FAIL  no $base_ref here: the branch must exist in this repository: the check needs the base to compare with"
        return 2
    fi
    base=$(g merge-base "$base_ref" HEAD) || { echo "FAIL  no merge-base of $base_ref and HEAD"; return 2; }
    echo "base: $(g rev-parse --short "$base") (merge-base of $base_ref and HEAD)"

    changed=$({
        g diff --name-only --no-renames "$base" HEAD
        g diff --name-only --no-renames HEAD
        g ls-files --others --exclude-standard
    } | sort -u)

    hook_files=""
    for f in $changed; do
        case "$f" in
            *[!A-Za-z0-9._/+-]*) echo "FAIL  odd file name: $f"; failed=1; continue ;;
        esac
        if echo "$f" | grep -Eq "$ALLOWED"; then
            continue
        fi
        if ! echo "$f" | grep -Eq "$HOOKS"; then
            echo "FORBIDDEN TOUCH: $f"
            failed=1
            continue
        fi
        # A hook: written down, and small.
        if [ ! -f "$doc" ] || ! grep -qF "$f" "$doc"; then
            echo "undocumented hook: $f (add a row to metal/UPSTREAM-TOUCHES.md)"
            failed=1
        fi
        lines=$(g diff --numstat --no-renames "$base" -- "$f" | awk '{ n += $1 + $2 } END { print n + 0 }')
        if [ "$lines" -gt "$MAX_HOOK_LINES" ]; then
            echo "hook too big: $f changes $lines lines (limit $MAX_HOOK_LINES)"
            failed=1
        fi
        hook_files="$hook_files $f"
    done

    # The document and the diff agree.
    if [ ! -f "$doc" ]; then
        echo "FAIL  $doc is missing"
        failed=1
    else
        declared=$(sed -n 's/^\*\*当前 \([0-9][0-9]*\) 处\*\*.*/\1/p' "$doc" | head -1)
        rows=$(grep -cE '^\| `[^`|]+:[0-9]+` \|' "$doc")
        n_hooks=$(echo $hook_files | wc -w | tr -d ' ')
        if [ -z "$declared" ]; then
            echo "FAIL  $doc has no '**当前 N 处**' line"
            failed=1
        elif [ "$declared" != "$rows" ]; then
            echo "FAIL  $doc says $declared places but has $rows table rows"
            failed=1
        elif [ "$rows" -lt "$n_hooks" ]; then
            echo "FAIL  $n_hooks hook files touched but only $rows places documented"
            failed=1
        fi
    fi

    if [ "$failed" = 0 ]; then
        echo "OK    no upstream file touched outside the documented hooks (hook files: $(echo $hook_files | wc -w | tr -d ' '))"
    fi
    return $failed
}

self_test() {
    tmp=$(mktemp -d "${TMPDIR:-/tmp}/check-touches.XXXXXX")
    trap 'rm -rf "$tmp"' EXIT
    status=0
    repo="$tmp/repo"

    mkrepo() {
        rm -rf "$repo"
        mkdir -p "$repo"
        (
            cd "$repo" || exit 1
            git init -q -b main .
            git config user.email t@t
            git config user.name t
            mkdir -p crates/victory-gpu/src tools/macos metal docs
            echo base > crates/a.rs 2>/dev/null || { mkdir -p crates; echo base > crates/a.rs; }
            seq 1 40 > crates/victory-gpu/src/lib.rs
            echo '#!/bin/sh' > tools/macos/victory.sh
            echo hello > README.md
            printf '**当前 0 处** (current: 0).\n\n| file:line | why | recall |\n|---|---|---|\n' > metal/UPSTREAM-TOUCHES.md
            git add -A
            git commit -q -m base
            git branch mac/livetr
            git checkout -q -b work
        )
    }
    expect() { # NAME WANT (reads the repo as set up by the caller)
        if REPO_DIR="$repo" check >/dev/null 2>&1; then got=pass; else got=fail; fi
        if [ "$got" = "$2" ]; then
            echo "self-test ok   $1 (expected $2)"
        else
            echo "self-test BAD  $1: expected $2, got $got"
            status=1
        fi
    }
    in_repo() { (cd "$repo" && "$@"); }
    commit() { in_repo git add -A && in_repo git commit -q -m "$1"; }
    doc_with() { # COUNT ROWS...
        n=$1; shift
        { printf '**当前 %s 处** (current: %s).\n\n| file:line | why | recall |\n|---|---|---|\n' "$n" "$n"; for r in "$@"; do printf '%s\n' "$r"; done; } > "$repo/metal/UPSTREAM-TOUCHES.md"
    }

    mkrepo
    expect "nothing changed" pass
    mkdir -p "$repo/metal/proto" "$repo/docs/metal" "$repo/tools/metal" "$repo/tests/metal"
    echo x > "$repo/metal/proto/a.rs"; echo x > "$repo/docs/metal/a.md"; echo x > "$repo/tools/metal/a.sh"; echo x > "$repo/tests/metal/a.rs"
    expect "new files in the allowed directories (uncommitted)" pass
    commit "metal files"
    expect "new files in the allowed directories (committed)" pass

    echo more >> "$repo/crates/a.rs"
    expect "uncommitted edit of an upstream file" fail
    commit "touch upstream"
    expect "committed edit of an upstream file" fail

    mkrepo; echo x > "$repo/crates/new.rs"
    expect "untracked new file in crates/" fail
    mkrepo; echo x >> "$repo/README.md"
    expect "edit of README.md" fail
    mkrepo; echo x > "$repo/Cargo.toml"
    expect "new root Cargo.toml" fail
    mkrepo; in_repo git rm -q crates/a.rs
    expect "deleting an upstream file" fail
    mkrepo; in_repo git mv crates/a.rs metal/a.rs; commit "move upstream file into metal/"
    expect "moving an upstream file into metal/ (the deletion counts)" fail

    mkrepo; echo 'mod in_place;' >> "$repo/crates/victory-gpu/src/lib.rs"
    expect "undocumented hook" fail
    doc_with 1 '| `crates/victory-gpu/src/lib.rs:41` | one mod line | delete when upstream has it |'
    expect "documented hook" pass
    doc_with 0
    expect "hook documented in the file but count says 0" fail
    doc_with 2 '| `crates/victory-gpu/src/lib.rs:41` | one mod line | x |'
    expect "count says 2 but one row" fail
    doc_with 1 '| `crates/victory-gpu/src/lib.rs:41` | one mod line | x |'
    commit "hook"
    expect "documented hook, committed" pass
    seq 100 106 >> "$repo/crates/victory-gpu/src/lib.rs"
    expect "hook of more than 5 lines" fail

    mkrepo; echo 'echo hi' >> "$repo/tools/macos/victory.sh"
    doc_with 1 '| `tools/macos/victory.sh:2` | one line | x |'
    expect "documented victory.sh hook" pass
    echo x >> "$repo/crates/victory-gpu/src/draw.rs" 2>/dev/null || { mkdir -p "$repo/crates/victory-gpu/src"; echo x > "$repo/crates/victory-gpu/src/draw.rs"; }
    expect "draw.rs is not a hook" fail

    mkrepo; rm "$repo/metal/UPSTREAM-TOUCHES.md"; in_repo git rm -q --cached metal/UPSTREAM-TOUCHES.md
    expect "document missing" fail

    mkrepo; in_repo git branch -D mac/livetr
    got=$(REPO_DIR="$repo" check >/dev/null 2>&1; echo $?)
    if [ "$got" = 2 ]; then echo "self-test ok   no mac/livetr (expected exit 2)"; else echo "self-test BAD  no mac/livetr: exit $got"; status=1; fi

    return $status
}

if [ "${1:-}" = "--self-test" ]; then
    self_test
    exit $?
fi
check
