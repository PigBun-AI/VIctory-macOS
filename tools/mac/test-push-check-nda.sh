#!/usr/bin/env bash
# tools/mac/test-push-check-nda.sh -- tests of the nda-gdkx path rules (push-check-nda.py, push-check.sh --nda-only, the pre-push hook).
# Everything happens in a throw-away repository with a local bare "fork". Exit 0 when every assertion holds.
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
T="$(mktemp -d "${TMPDIR:-/tmp}/nda-test.XXXXXX")"
trap 'rm -rf "$T"' EXIT
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_SYSTEM=/dev/null
FAILS=0; PASSES=0
ok()   { PASSES=$((PASSES + 1)); echo "  ok   $*"; }
fail() { FAILS=$((FAILS + 1)); echo "  FAIL $*"; }

git init -q --bare "$T/fork.git"
R="$T/repo"; git init -q -b main "$R"; cd "$R" || exit 1
git config user.name "Test Dev"; git config user.email "dev@example.invalid"
mkdir -p tools/mac crates/hle/src
cp "$HERE/push-check.sh" "$HERE/push-check-nda.py" "$HERE/install-hooks.sh" tools/mac/
echo base > README.md; git add -A; git commit -q -m base
git remote add fork "$T/fork.git"; git push -q fork main 2>&1 | sed 's/^/  /'; git fetch -q fork
mkdir -p .git/hooks
printf '#!/bin/sh\necho "orig args=$#" > "%s/orig.out"\ncat >> "%s/orig.out"\nexit 0\n' "$T" "$T" > .git/hooks/pre-push; chmod +x .git/hooks/pre-push

branch() { git checkout -q -B "$1" fork/main; }
commit() { git add -A; git commit -q -m "${1:-change}"; }
nda() { bash tools/mac/push-check.sh --nda-only "$1" 2>&1; }
blocked() { out="$(nda "$2")"; rc=$?; if [ "$rc" -ne 0 ] && echo "$out" | grep -q 'BLOCK (nda-gdkx)'; then ok "blocked: $1"; else fail "NOT blocked: $1 (rc=$rc)"; echo "$out" | sed 's/^/       /'; fi; }
passes()  { [ "$(git rev-list --count "$2" --not --remotes=fork)" -ge 1 ] || { fail "setup: $2 has no commit"; return; }; out="$(nda "$2")"; rc=$?; if [ "$rc" -eq 0 ] && ! echo "$out" | grep -q BLOCK; then ok "passes: $1"; else fail "NOT passed: $1 (rc=$rc)"; echo "$out" | sed 's/^/       /'; fi; }

echo "== must be blocked"
for n in d3d12_x.h d3d12x_extra.h xg.hpp xgmath.h XGame3.h foo_xs.h pix3_xbox.h dstorage_xs.h D3D12_XS.H; do
    branch "b-$n"; echo "x" > "$n"; commit "add $n"; blocked "header $n" "b-$n"
done
branch b-priv;  mkdir -p private; echo x > private/foo; commit "add";            blocked "private/foo" b-priv
branch b-priv2; mkdir -p a/Private/b; echo x > a/Private/b/c.txt; commit "add"; blocked "nested Private/ segment" b-priv2
branch b-dir1;  mkdir -p third_party/GDKX; echo x > third_party/GDKX/readme.txt; commit "add"; blocked "directory gdkx" b-dir1
branch b-dir2;  mkdir -p vendor/gxdk-tools; echo x > vendor/gxdk-tools/a.txt; commit "add"; blocked "directory gxdk" b-dir2
echo "== must pass"
branch ok1; mkdir -p crates/hle/src; echo "pub fn f() {}" > crates/hle/src/xg.rs; commit "hle: xg"; passes "xg.rs" ok1
branch ok2; mkdir -p crates/hle/src; echo "pub fn f() {}" > crates/hle/src/d3d12_x.rs; echo s > crates/hle/src/d3d12_x.dll.txt; commit "hle: d3d12_x.dll"; passes "d3d12_x.dll HLE files" ok2
branch ok3; mkdir -p crates/hle/src; for n in xgame xgameui dstorage private; do echo "pub fn f() {}" > "crates/hle/src/$n.rs"; done; commit "hle: more"; passes "xgame*.rs, dstorage.rs, private.rs" ok3
echo "== full push-check cannot be told to skip it"
out="$(bash tools/mac/push-check.sh --skip-policy --no-words --strict --windows-review b-d3d12_x.h 2>&1)"; rc=$?
[ "$rc" -eq 1 ] && echo "$out" | grep -q 'BLOCK (nda-gdkx)' && ok "options do not switch the class off" || fail "options bypassed (rc=$rc)"
echo "== hook"
bash tools/mac/install-hooks.sh >/dev/null 2>&1 && bash tools/mac/install-hooks.sh >/dev/null 2>&1 || fail "install-hooks.sh"
[ "$(grep -c '^# >>> gdkx-guard' .git/hooks/pre-push)" = 1 ] && ok "installed twice: one managed block" || fail "block count"
grep -q 'orig args' .git/hooks/pre-push && ok "original pre-push content kept" || fail "original content lost"
git checkout -q ok1; rm -f "$T/orig.out"
git push -q fork ok1:refs/heads/ok1 >"$T/p.out" 2>&1 && ok "clean push goes through" || { fail "clean push refused"; cat "$T/p.out"; }
grep -q 'refs/heads/ok1' "$T/orig.out" 2>/dev/null && grep -q 'orig args=2' "$T/orig.out" && ok "original hook still gets its arguments and the ref list" || fail "original hook not run"
for b in b-d3d12_x.h b-priv; do
    git push -q fork "$b:refs/heads/$b" >"$T/p.out" 2>&1 && fail "hook let $b through" || { git --git-dir="$T/fork.git" rev-parse -q --verify "refs/heads/$b" >/dev/null && fail "$b arrived" || ok "hook refused $b"; }
done
echo
echo "test-push-check-nda: $PASSES passed, $FAILS failed"
[ "$FAILS" -eq 0 ]
