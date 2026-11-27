#!/usr/bin/env bash
# tools/mac/sync-windows.sh -- pull the newest Windows development branch (read
# only), merge it into mac/livetr, build + test, and write a report.
#
#   tools/mac/sync-windows.sh [options]          (see --help, docs/mac/SYNC.md)
#
# Windows side: read only. The only things run there are `git ls-remote` and
# `git fetch` (the server runs `git upload-pack .`, nothing else). Nothing is
# pushed to Windows or to any upstream; --push-fork (off by default) pushes the
# target branch to the `fork` remote (PigBun-AI/VIctory-macOS) and nowhere else.
#
# Secrets: none in this file. Run it under credential_exec; it reads the Windows
# login from the environment (WIN_USER/WIN_PASSWORD, else the variables that
# credential-7ed4105d... exports: PIGBUN_CRED_WINDOWS_7ED4_USER / _PASSWORD).
# ssh asks this same script for the password (SSH_ASKPASS) and the script only
# echoes the environment variable: nothing is written to disk.
#
# Exit codes: 0 ok / already up to date / dry run   1 refused or error
#             2 merge conflict (merge aborted, tree as before)
#             3 held: branch lineage differs or diverges a lot, needs a decision
#             4 merged, but build/test failed   5 could not reach Windows

# ---- askpass mode (ssh calls us back) ---------------------------------------
if [ "${SYNC_ASKPASS_MODE:-}" = 1 ]; then
    printf '%s\n' "${SYNC_WIN_PW:-}"
    exit 0
fi

set -u -o pipefail
SELF="$(cd "$(dirname "$0")" && pwd)/$(basename "$0")"
SCRIPT_DIR="$(dirname "$SELF")"

# ---- configuration (env overrides; options below override these) -----------
# Site-specific values (the Windows host and repo, the report directory) are not kept in
# this file: set them in the environment, or in tools/mac/sync.local.env (untracked
# and ignored by git), which is sourced here when it exists. See docs/mac/SYNC.md.
[ -f "$SCRIPT_DIR/sync.local.env" ] && . "$SCRIPT_DIR/sync.local.env"
WIN_HOST="${SYNC_WIN_HOST:-}"
WIN_REPO="${SYNC_WIN_REPO:-}"
WIN_USER="${WIN_USER:-${PIGBUN_CRED_WINDOWS_7ED4_USER:-}}"
WIN_PASSWORD="${WIN_PASSWORD:-${PIGBUN_CRED_WINDOWS_7ED4_PASSWORD:-}}"
SYNC_DIR="${SYNC_DIR:-}"
# Branches never picked automatically (extended regex on the branch name):
# main/master, backup/temporary branches, the sync helper branches.
DEFAULT_EXCLUDE='^(main|master|HEAD)$|^(tmp|temp|backup|bak|scratch|old|archive|sync)/|(^|[/_.-])(backup|bak|tmp|temp|old)([/_.-]|$)'
EXCLUDE="${SYNC_EXCLUDE_REGEX:-$DEFAULT_EXCLUDE}"
MAX_AHEAD="${SYNC_MAX_AHEAD:-300}"      # more new commits than this: hold, ask
LIST_MAX="${SYNC_LIST_MAX:-40}"         # commits listed one by one in the report
TOP_N=5
FORK_REMOTE="${SYNC_FORK_REMOTE:-fork}"
CHECK_TIMEOUT="${SYNC_CHECK_TIMEOUT:-7200}"

WT="${SYNC_WORKTREE:-}"
TARGET="${SYNC_TARGET_BRANCH:-mac/livetr}"
BRANCH=""; DRY=0; PUSH_FORK=0; ROLLBACK=0; FORCE=0; NOFETCH=0; NOCHECK=0; CHECK_ANYWAY=0
STATE_FILE=""; REPORT_FILE=""; REPORT_DIR=""; MAIN_REPO=""

usage() { sed -n '2,/^#             4 merged/p' "$SELF" | sed 's/^# \{0,1\}//' ; cat <<'EOF'

Options
  --branch NAME          merge this Windows branch instead of the most recently committed one
  --worktree PATH        worktree to merge in (default: the checkout this script lives in)
  --target-branch NAME   branch that worktree must be on (default mac/livetr)
  --dry-run              fetch and analyse only: no merge, no build, no state change
  --exclude REGEX        also skip branches matching REGEX (SYNC_EXCLUDE_REGEX replaces the default list)
  --force-switch         merge even when the branch is on a different lineage / diverges a lot
  --rollback-on-fail     if build/test fails, reset the target branch back to the old HEAD
  --no-check             merge only, skip tools/mac/check.sh
  --check-anyway         run the checks even when there was nothing new to merge
  --push-fork            after a green run push the target branch to remote `fork` (default: off)
  --no-fetch             do not contact Windows; use the refs/remotes/win-all/* already here
  --report FILE | --report-dir DIR   where the Markdown report goes (default $SYNC_DIR/<date-time>.md)
  --state FILE           state file (default $SYNC_DIR/state); records the last branch/commit merged
  --top N                candidates to print (default 5)
  -h, --help

Required settings (environment, or tools/mac/sync.local.env):
  SYNC_WIN_HOST   Windows machine to fetch from (not needed with --no-fetch)
  SYNC_WIN_REPO   path of the Windows repo, as git on Windows sees it (not needed with --no-fetch)
  SYNC_DIR        directory for reports and the state file (not needed with --state and --report/--report-dir)
  WIN_USER / WIN_PASSWORD   Windows login (or PIGBUN_CRED_WINDOWS_7ED4_USER / _PASSWORD)
EOF
}

while [ $# -gt 0 ]; do
    case "$1" in
        --branch) BRANCH="${2:?}"; shift 2 ;;
        --worktree) WT="${2:?}"; shift 2 ;;
        --target-branch) TARGET="${2:?}"; shift 2 ;;
        --dry-run) DRY=1; shift ;;
        --exclude) EXCLUDE="$EXCLUDE|${2:?}"; shift 2 ;;
        --force-switch) FORCE=1; shift ;;
        --rollback-on-fail) ROLLBACK=1; shift ;;
        --no-check) NOCHECK=1; shift ;;
        --check-anyway) CHECK_ANYWAY=1; shift ;;
        --push-fork) PUSH_FORK=1; shift ;;
        --no-fetch) NOFETCH=1; shift ;;
        --report) REPORT_FILE="${2:?}"; shift 2 ;;
        --report-dir) REPORT_DIR="${2:?}"; shift 2 ;;
        --state) STATE_FILE="${2:?}"; shift 2 ;;
        --top) TOP_N="${2:?}"; shift 2 ;;
        --main-repo) MAIN_REPO="${2:?}"; shift 2 ;;
        -h|--help) usage; exit 0 ;;
        *) echo "unknown option: $1 (try --help)" >&2; exit 1 ;;
    esac
done

# ---- site-specific settings must come from the environment -------------------
MISSING=""
if [ "$NOFETCH" = 0 ]; then
    [ -n "$WIN_HOST" ] || MISSING="$MISSING SYNC_WIN_HOST"
    [ -n "$WIN_REPO" ] || MISSING="$MISSING SYNC_WIN_REPO"
fi
if [ -z "$SYNC_DIR" ] && { [ -z "$STATE_FILE" ] || { [ -z "$REPORT_FILE" ] && [ -z "$REPORT_DIR" ]; }; }; then
    MISSING="$MISSING SYNC_DIR"
fi
if [ -n "$MISSING" ]; then
    echo "sync-windows.sh: not set:$MISSING" >&2
    echo "  Export them or put them in $SCRIPT_DIR/sync.local.env (see docs/mac/SYNC.md):" >&2
    echo "    SYNC_WIN_HOST=<Windows host>  SYNC_WIN_REPO=<repo path on Windows>  SYNC_DIR=<report/state directory>" >&2
    exit 1
fi

# ---- output helpers ---------------------------------------------------------
if [ -t 1 ]; then RED=$'\033[1;31m'; GRN=$'\033[32m'; YEL=$'\033[1;33m'; RST=$'\033[0m'; else RED=; GRN=; YEL=; RST=; fi
say()  { printf '%s\n' "$*"; }
warn() { printf '%s%s%s\n' "$YEL" "$*" "$RST"; }
bad()  { printf '%s%s%s\n' "$RED" "$*" "$RST"; }
STAMP="$(date +%Y%m%d-%H%M%S)"
NOW_H="$(date '+%Y-%m-%d %H:%M')"
TMPD="$(mktemp -d -t sync-windows.XXXXXX)"
BODY="$TMPD/body.md"; : > "$BODY"
R()  { printf '%s\n' "$*" >> "$BODY"; }
STATUS="ERROR"; HEADLINE=""; EXITCODE=1; LOCKDIR=""; REPORT_WRITTEN=0
mktmp() { mktemp "$TMPD/f.XXXXXX"; }

finish() { # finish EXIT_CODE STATUS HEADLINE
    EXITCODE="$1"; STATUS="$2"; HEADLINE="$3"
    exit "$EXITCODE"
}
die() { # die MESSAGE [code] [status]
    bad "sync-windows: $1"
    R "## 中止"; R ""; R "$1"; R ""
    finish "${2:-1}" "${3:-ERROR}" "$1"
}

write_report() {
    [ "$REPORT_WRITTEN" = 1 ] && return
    REPORT_WRITTEN=1
    local out icon
    if [ -n "$REPORT_FILE" ]; then out="$REPORT_FILE"
    else out="${REPORT_DIR:-$SYNC_DIR}/$STAMP.md"; fi
    mkdir -p "$(dirname "$out")" 2>/dev/null || return
    case "$STATUS" in
        OK|UPTODATE|DRYRUN) icon="✅" ;;
        HELD) icon="🟠" ;;
        *) icon="🔴" ;;
    esac
    {
        printf '# Mac 同步报告 %s\n\n' "$NOW_H"
        printf '%s **%s** — %s\n\n' "$icon" "$STATUS" "$HEADLINE"
        cat "$BODY"
    } > "$out" 2>/dev/null && say "report: $out"
}
cleanup() {
    local rc=$?
    # an unexpected exit (set by neither finish nor die) still leaves a record
    [ "$STATUS" = ERROR ] && [ -z "$HEADLINE" ] && HEADLINE="unexpected exit ($rc)"
    write_report
    [ -n "$LOCKDIR" ] && rm -rf "$LOCKDIR"
    rm -rf "$TMPD"
}
trap cleanup EXIT
trap 'bad "interrupted"; HEADLINE="interrupted"; exit 130' INT TERM

# ---- locate worktree and repos ---------------------------------------------
[ -n "$WT" ] || WT="$(git -C "$SCRIPT_DIR" rev-parse --show-toplevel 2>/dev/null)" || die "cannot find the worktree; pass --worktree PATH"
WT="$(cd "$WT" 2>/dev/null && pwd -P)" || die "worktree $WT does not exist"
g() { git -C "$WT" "$@"; }
g rev-parse --git-dir >/dev/null 2>&1 || die "$WT is not a git worktree"
COMMON="$(g rev-parse --path-format=absolute --git-common-dir)"
[ -n "$MAIN_REPO" ] || MAIN_REPO="$(dirname "$COMMON")"
[ -n "$STATE_FILE" ] || STATE_FILE="$SYNC_DIR/state"
mkdir -p "$(dirname "$STATE_FILE")" 2>/dev/null

R "- 模式: $([ $DRY = 1 ] && echo 'dry-run (只拉取和分析)' || echo '合并 + 验证')"
R "- worktree: \`$WT\` (目标分支 \`$TARGET\`)"
R "- 主仓库: \`$MAIN_REPO\` (fetch 在这里做, refs 在 refs/remotes/win-all/ 和 refs/remotes/win/)"

# one run at a time per state file
LOCKDIR="$STATE_FILE.lock"
if ! mkdir "$LOCKDIR" 2>/dev/null; then
    oldpid="$(cat "$LOCKDIR/pid" 2>/dev/null || true)"
    if [ -n "$oldpid" ] && kill -0 "$oldpid" 2>/dev/null; then
        LOCKDIR=""; die "another sync-windows run is active (pid $oldpid)"
    fi
    rm -rf "$LOCKDIR"; mkdir "$LOCKDIR" || { LOCKDIR=""; die "cannot take the lock $LOCKDIR"; }
fi
echo $$ > "$LOCKDIR/pid"

# ---- worktree preconditions -------------------------------------------------
CUR_BRANCH="$(g symbolic-ref --short -q HEAD || true)"
[ "$CUR_BRANCH" = "$TARGET" ] || die "worktree is on '${CUR_BRANCH:-detached HEAD}', not on $TARGET: refusing"
for p in MERGE_HEAD CHERRY_PICK_HEAD REVERT_HEAD REBASE_HEAD rebase-merge rebase-apply; do
    [ -e "$(g rev-parse --path-format=absolute --git-path "$p")" ] && die "a $p operation is in progress in $WT: refusing"
done
DIRTY="$(g status --porcelain --untracked-files=no)"
UNTRACKED="$(g status --porcelain --untracked-files=normal | grep -c '^??' || true)"
if [ -n "$DIRTY" ]; then
    if [ $DRY = 1 ]; then
        warn "worktree has uncommitted tracked changes (fine for --dry-run, a real run would refuse)"
        R "- ⚠️ 工作区有未提交的改动 (dry-run 不受影响, 真正合并会拒绝):"; R '```'; R "$DIRTY"; R '```'
    else
        R "工作区不干净:"; R '```'; R "$DIRTY"; R '```'
        die "worktree has uncommitted changes to tracked files; commit or stash them first (refusing to merge)"
    fi
fi
[ "${UNTRACKED:-0}" -gt 0 ] && warn "note: $UNTRACKED untracked file(s) in the worktree (ignored; git itself refuses a merge that would overwrite one)"
OLD_HEAD="$(g rev-parse HEAD)"

# ---- Windows access (read only) --------------------------------------------
UPLOAD_PACK="git -C $WIN_REPO upload-pack . & rem"
win_git() { # win_git <git args...>: run git against Windows with the askpass trick
    env SSH_ASKPASS="$SELF" SSH_ASKPASS_REQUIRE=force DISPLAY="${DISPLAY:-x}" \
        SYNC_ASKPASS_MODE=1 SYNC_WIN_PW="$WIN_PASSWORD" GIT_TERMINAL_PROMPT=0 \
        GIT_SSH_COMMAND="ssh -o PreferredAuthentications=password,keyboard-interactive -o PubkeyAuthentication=no -o ConnectTimeout=10 -o NumberOfPasswordPrompts=1" \
        git --git-dir="$COMMON" -c gc.auto=0 -c maintenance.auto=false "$@"
}
REMOTE_NAMES="$(mktmp)"
if [ $NOFETCH = 0 ]; then
    [ -n "$WIN_USER" ] && [ -n "$WIN_PASSWORD" ] || die "no Windows login in the environment: run under credential_exec with credential-7ed4105d97af436baf552fc3f7863f41 (PIGBUN_CRED_WINDOWS_7ED4_USER/_PASSWORD) or set WIN_USER/WIN_PASSWORD" 5 ERROR
    WIN_URL="ssh://$WIN_USER@$WIN_HOST/x"
    say "== Windows ($WIN_HOST): ls-remote refs/heads/*"
    LS="$(mktmp)"
    win_git ls-remote --heads --upload-pack="$UPLOAD_PACK" "$WIN_URL" > "$LS" 2> "$LS.err" \
        || { sed 's/^/  /' "$LS.err" >&2; rm -f "$LS.err"; die "ls-remote on Windows failed ($WIN_HOST)" 5 ERROR; }
    rm -f "$LS.err"
    [ -s "$LS" ] || die "Windows returned no branches; not touching refs/remotes/win-all" 5 ERROR
    awk '{sub(/^refs\/heads\//,"",$2); print $2}' "$LS" | sort > "$REMOTE_NAMES"
    REMOTE_SHAS="$(awk '{sub(/^refs\/heads\//,"",$2); print $1" "$2}' "$LS" | sort)"
    LOCAL_SHAS="$(git --git-dir="$COMMON" for-each-ref --format='%(objectname) %(refname:strip=3)' refs/remotes/win-all | sort)"
    NBR="$(wc -l < "$REMOTE_NAMES" | tr -d ' ')"
    if [ "$REMOTE_SHAS" = "$LOCAL_SHAS" ]; then
        say "   $NBR branches, all already fetched"
    else
        say "== fetch (read only, upload-pack) -> refs/remotes/win-all/*"
        win_git fetch --no-tags --prune --upload-pack="$UPLOAD_PACK" "$WIN_URL" '+refs/heads/*:refs/remotes/win-all/*' > "$LS.out" 2>&1 \
            || { sed 's/^/  /' "$LS.out" >&2; rm -f "$LS.out"; die "fetch from Windows failed" 5 ERROR; }
        say "   $(grep -c -E '^ [*+ -] |->' "$LS.out" || true) ref update(s)"
        rm -f "$LS.out"
    fi
else
    say "== --no-fetch: using the refs/remotes/win-all/* already here"
    : > "$REMOTE_NAMES"
fi

# ---- candidate branches ----------------------------------------------------
CAND="$(mktmp)"
git --git-dir="$COMMON" for-each-ref --sort=-committerdate \
    --format='%(committerdate:unix)|%(committerdate:format-local:%Y-%m-%d %H:%M)|%(objectname)|%(refname:strip=3)|%(subject)' \
    refs/remotes/win-all | while IFS='|' read -r ts dt sha name subj; do
        if [ -s "$REMOTE_NAMES" ] && ! grep -qxF -- "$name" "$REMOTE_NAMES"; then continue; fi
        if printf '%s\n' "$name" | grep -Eq -- "$EXCLUDE"; then continue; fi
        printf '%s|%s|%s|%s|%s\n' "$ts" "$dt" "$sha" "$name" "$subj"
    done > "$CAND"
[ -s "$CAND" ] || die "no candidate branch left after excluding /$EXCLUDE/"

say "== candidates (newest commit first; excluded: /$EXCLUDE/)"
R ""; R "## 候选分支 (按最近提交时间, top $TOP_N)"; R ""
R "排除规则 (可配置: \`SYNC_EXCLUDE_REGEX\` / \`--exclude\`): \`$EXCLUDE\`"; R ""
R "| # | 分支 | 最近提交 | HEAD | 说明 |"; R "|---|---|---|---|---|"
i=0
while IFS='|' read -r ts dt sha name subj; do
    i=$((i+1)); [ $i -gt "$TOP_N" ] && break
    [ ${#subj} -gt 100 ] && subj="${subj:0:100}…"
    mark=" "; [ "$i" = 1 ] && [ -z "$BRANCH" ] && mark="*"
    printf ' %s %d. %-34s %s  %s  %s\n' "$mark" "$i" "$name" "$dt" "${sha:0:9}" "$subj"
    R "| $i | \`$name\` | $dt | \`${sha:0:9}\` | $subj |"
done < "$CAND"

if [ -n "$BRANCH" ]; then
    LINE="$(awk -F'|' -v b="$BRANCH" '$4==b{print; exit}' "$CAND")"
    if [ -z "$LINE" ]; then   # excluded by the regex, or not on the remote: --branch still allows excluded names that exist
        sha="$(git --git-dir="$COMMON" rev-parse -q --verify "refs/remotes/win-all/$BRANCH^{commit}" || true)"
        [ -n "$sha" ] || die "branch '$BRANCH' is not among the Windows branches fetched (refs/remotes/win-all/$BRANCH)"
        [ -s "$REMOTE_NAMES" ] && ! grep -qxF -- "$BRANCH" "$REMOTE_NAMES" && die "branch '$BRANCH' is no longer on Windows"
        dt="$(git --git-dir="$COMMON" log -1 --date=format-local:'%Y-%m-%d %H:%M' --format=%cd "$sha")"
        LINE="0|$dt|$sha|$BRANCH|$(git --git-dir="$COMMON" log -1 --format=%s "$sha")"
    fi
    PICK_WHY="--branch"
else
    LINE="$(head -n 1 "$CAND")"; PICK_WHY="最近提交时间最新"
fi
SEL_NAME="$(printf '%s' "$LINE" | awk -F'|' '{print $4}')"
SEL_SHA="$(printf '%s' "$LINE" | awk -F'|' '{print $3}')"
SEL_DATE="$(printf '%s' "$LINE" | awk -F'|' '{print $2}')"
NEW="$SEL_SHA"
WINREF="refs/remotes/win/$(printf '%s' "$SEL_NAME" | tr '/' '-')"
# convenience ref win/<name with / as ->; a dry run leaves the existing refs alone
if [ $DRY = 0 ]; then git --git-dir="$COMMON" update-ref "$WINREF" "$NEW" 2>/dev/null || warn "could not update $WINREF"; fi
say "selected: $SEL_NAME @ ${NEW:0:9} ($SEL_DATE) [$PICK_WHY]; ref $WINREF"

# ---- state: did the branch change? -----------------------------------------
st_get() { [ -f "$STATE_FILE" ] && sed -n "s/^$1=//p" "$STATE_FILE" | tail -n 1; }
LAST_BRANCH="$(st_get last_branch || true)"; LAST_COMMIT="$(st_get last_commit || true)"; LAST_RUN="$(st_get last_run || true)"
SWITCHED=0
if [ -z "$LAST_BRANCH" ]; then
    warn "no earlier run recorded in $STATE_FILE (first run)"
elif [ "$LAST_BRANCH" != "$SEL_NAME" ]; then
    SWITCHED=1
    bad "################################################################"
    bad "##  BRANCH CHANGED: last '$LAST_BRANCH' -> now '$SEL_NAME'"
    bad "##  this may need a decision (is it the line the user means?)"
    bad "################################################################"
fi
R ""; R "## 概览"; R ""
R "- 选中分支: **\`$SEL_NAME\`** @ \`${NEW:0:9}\` ($SEL_DATE), 选择方式: $PICK_WHY; 本地 ref \`${WINREF#refs/remotes/}\`"
R "- 上次: $([ -n "$LAST_BRANCH" ] && echo "\`$LAST_BRANCH\` @ \`${LAST_COMMIT:0:9}\` ($LAST_RUN)" || echo '(没有记录, 第一次运行)')"
if [ $SWITCHED = 1 ]; then
    R ""; R "> 🔴 **分支变了: \`$LAST_BRANCH\` → \`$SEL_NAME\`。可能需要用户决策。**"; R ""
fi

# ---- analysis ---------------------------------------------------------------
if g merge-base --is-ancestor "$NEW" HEAD 2>/dev/null; then
    R "- $TARGET 已经包含 \`${NEW:0:9}\`: 没有新提交"
    say "already up to date: $TARGET contains $SEL_NAME@${NEW:0:9}"
    UPTODATE=1
else
    UPTODATE=0
fi
BASE="$(g merge-base HEAD "$NEW" 2>/dev/null || true)"
AHEAD="$(g rev-list --count "HEAD..$NEW" 2>/dev/null || echo 0)"
# commits in HEAD that are neither in NEW nor ours (mac:): Windows-side history NEW does not have
FOREIGN="$(mktmp)"
g log --no-merges --format='%h %s' "$NEW..HEAD" | awk '$0 !~ /^[0-9a-f]+ mac:/' > "$FOREIGN"
NFOREIGN="$(wc -l < "$FOREIGN" | tr -d ' ')"
OURS_N="$(g log --no-merges --format='%s' "$NEW..HEAD" | grep -c '^mac:' || true)"
R "- $TARGET: \`${OLD_HEAD:0:9}\` ($(g log -1 --format=%s HEAD | cut -c1-60))"
if [ -n "$BASE" ]; then
    R "- merge-base: \`${BASE:0:9}\` $(g log -1 --date=format-local:'%Y-%m-%d %H:%M' --format='(%cd) %s' "$BASE" | cut -c1-100)"
else
    R "- merge-base: **没有共同祖先**"
fi
R "- 新分支比 $TARGET 多 **$AHEAD** 个提交; $TARGET 有 $OURS_N 个 \`mac:\` 提交不在新分支里 (正常); 有 $NFOREIGN 个非 \`mac:\` 提交不在新分支里$([ "$NFOREIGN" -gt 0 ] && echo ' (🔴 谱系不同)')"
BASE9="${BASE:0:9}"; BASE9="${BASE9:-none}"
say "merge-base: $BASE9   ahead: $AHEAD   Windows-side commits missing from the new branch: $NFOREIGN"

# mac: patch file set (files our mac: commits touch)
MACFILES="$(mktmp)"
g log --no-merges --format='%H%x09%s' HEAD "^$NEW" | awk -F'\t' '$2 ~ /^mac:/ {print $1}' | while read -r c; do
    g diff-tree --no-commit-id --name-only -r "$c"
done | sort -u > "$MACFILES"

# commit analysis -> TSV: sha9 subj flags nfiles files machits
analyze_range() { # $1 = range, $2 = out tsv
    local dset raw; dset="$(mktmp)"; raw="$(mktmp)"
    g log --format=%H -G'D3D12|d3d12' "$1" -- crates/victory-gpu crates/victory-hle crates/victory-loader > "$dset"
    g log --reverse --name-only --format='@@%H%x09%s' "$1" > "$raw"
    awk -F'\t' -v macfile="$MACFILES" -v dfile="$dset" '
        BEGIN { while ((getline l < macfile) > 0) mac[l] = 1; while ((getline l < dfile) > 0) ds[l] = 1 }
        function flush(   fl) {
            if (sha == "") return
            fl = ""
            if (mc) fl = fl "M"; if (gp) fl = fl "G"; if (d3) fl = fl "D"; if (ld) fl = fl "L"; if (hl) fl = fl "H"
            printf "%s\t%s\t%s\t%d\t%s\t%s\n", substr(sha,1,9), subj, fl, nf, files, hits
        }
        /^@@/ { flush(); sha = substr($1, 3); subj = $2; files = ""; hits = ""; nf = 0; mc = gp = d3 = ld = hl = 0; nh = 0
                if (subj ~ /[dD]3[dD]12/ || (sha in ds)) d3 = 1; next }
        $0 == "" { next }
        { f = $0; nf++
          if (nf <= 6) files = files (files == "" ? "" : ", ") f
          if (f in mac) { mc = 1; nh++; if (nh <= 5) hits = hits (hits == "" ? "" : ", ") f }
          if (f ~ /^crates\/victory-gpu\//) gp = 1
          if (f ~ /^crates\/victory-loader\//) ld = 1
          if (f ~ /^crates\/victory-hle\//) hl = 1
          if (f ~ /[dD]3[dD]12/) d3 = 1 }
        END { flush() }' "$raw" > "$2"
}
COMMITS="$(mktmp)"
[ $UPTODATE = 0 ] && analyze_range "HEAD..$NEW" "$COMMITS"

report_commits() { # sections of the report, from $COMMITS
    local n; n="$(wc -l < "$COMMITS" | tr -d ' ')"
    R ""; R "## 新增提交 ($n)"; R ""
    if [ "$n" = 0 ]; then R "(无)"; return; fi
    awk -F'\t' -v max="$LIST_MAX" 'NR<=max { f=$3; printf "- `%s` %s%s\n", $1, $2, (f==""?"":"  [" f "]") }
        END { if (NR>max) printf "- … 另有 %d 个, 见 `git log HEAD..<新分支>`\n", NR-max }' "$COMMITS" >> "$BODY"
    R ""; R "标记: M=改了 mac: 补丁碰过的文件, G=victory-gpu, D=D3D12 相关, L=victory-loader, H=victory-hle"
    R ""; R "### 触及 Mac 补丁文件的提交 (合并时最可能冲突/需要复查)"; R ""
    if awk -F'\t' '$3 ~ /M/ {c++} END{exit !c}' "$COMMITS"; then
        awk -F'\t' '$3 ~ /M/ { printf "- `%s` %s\n  - 补丁文件: %s\n", $1, $2, $6 }' "$COMMITS" | head -n 120 >> "$BODY"
    else R "(无)"; fi
    R ""; R "### victory-gpu (D3D12 相关标 **D3D12**)"; R ""
    if awk -F'\t' '$3 ~ /G/ {c++} END{exit !c}' "$COMMITS"; then
        awk -F'\t' '$3 ~ /G/ { printf "- %s`%s` %s\n", ($3 ~ /D/ ? "**D3D12** " : ""), $1, $2 }' "$COMMITS" | head -n 80 >> "$BODY"
    else R "(无)"; fi
    R ""; R "### victory-loader"; R ""
    if awk -F'\t' '$3 ~ /L/ {c++} END{exit !c}' "$COMMITS"; then
        awk -F'\t' '$3 ~ /L/ { printf "- `%s` %s\n", $1, $2 }' "$COMMITS" | head -n 60 >> "$BODY"
    else R "(无)"; fi
    R ""; R "### victory-hle"; R ""
    if awk -F'\t' '$3 ~ /H/ {c++} END{exit !c}' "$COMMITS"; then
        awk -F'\t' '$3 ~ /H/ { printf "- %s`%s` %s\n", ($3 ~ /D/ ? "**D3D12** " : ""), $1, $2 }' "$COMMITS" | head -n 60 >> "$BODY"
    else R "(无)"; fi
}

# which Mac patches (docs/mac/PATCHES.md) / mac: commits relate to a file
patch_info() { # patch_info FILE -> markdown bullet(s)
    local f="$1" base crate keys secs commits
    base="$(basename "$f")"; crate="$(printf '%s' "$f" | sed -n 's#^crates/\([^/]*\)/.*#\1#p')"
    keys="$f|${f#crates/}|$(printf '%s' "${f#crates/}" | sed 's#/src/#/#')"
    [ -n "$crate" ] && keys="$keys|$crate/$base"
    case "$base" in lib.rs|mod.rs|main.rs|Cargo.toml) ;; *) keys="$keys|$base" ;; esac
    secs="$(g show "HEAD:docs/mac/PATCHES.md" 2>/dev/null | awk -v keys="$keys" '
        BEGIN { n = split(keys, k, "|") }
        /^##/ { if (cur != "" && hit) printf "%s ", cur; cur = ($1 == "###") ? $2 : ""; hit = 0; next }
        { for (i = 1; i <= n; i++) if (k[i] != "" && index($0, k[i])) hit = 1 }
        END { if (cur != "" && hit) printf "%s ", cur }' | sed 's/ *$//; s/ /, /g')"
    commits="$(g log --no-merges --format='%h %s' -n 8 HEAD "^$NEW" -- "$f" | awk '$2 ~ /^mac:/' | head -n 3 | sed 's/^/`/; s/ /` /')"
    printf -- '- `%s`: ' "$f"
    if [ -n "$secs" ]; then printf 'PATCHES.md 条目 **%s**' "$secs"; else printf 'PATCHES.md 里没有提到'; fi
    if [ -n "$commits" ]; then printf '; 相关 mac: 提交: %s' "$(printf '%s' "$commits" | tr '\n' ';')"; else printf '; 没有 mac: 提交改过它 (可能是 Windows 线自己的改动或合并基线的问题)'; fi
    printf '\n'
}

predict_conflicts() { # sets PRED_FILES (newline list) and PRED_RC
    PRED_FILES=""; PRED_RC=0
    local out; out="$(g merge-tree --write-tree --name-only --no-messages HEAD "$NEW" 2>&1)"; PRED_RC=$?
    if [ $PRED_RC -eq 1 ]; then PRED_FILES="$(printf '%s\n' "$out" | sed 1d | sed '/^$/d')"
    elif [ $PRED_RC -ne 0 ]; then PRED_FILES=""; PRED_RC=9; fi
}

# ---- up to date -------------------------------------------------------------
if [ $UPTODATE = 1 ] && [ $CHECK_ANYWAY = 0 ]; then
    if [ $SWITCHED = 1 ]; then   # the "new" branch is only an ancestor of ours: nothing merged, so keep the old record and keep warning
        R ""; R "> 分支变了, 但 \`$SEL_NAME\` 已经是 $TARGET 的祖先 (没有合并任何东西)。状态文件没有更新, 下次仍会提示。"
    elif [ $DRY = 0 ]; then
        printf 'last_branch=%s\nlast_commit=%s\nlast_head=%s\nlast_run=%s\n' "$SEL_NAME" "$NEW" "$OLD_HEAD" "$NOW_H" > "$STATE_FILE"
    fi
    R ""; R "没有需要合并的新提交, 未改动任何东西$([ $DRY = 1 ] && echo ' (dry-run)')。"
    say "nothing to merge"
    finish 0 UPTODATE "$TARGET 已包含 $SEL_NAME@${NEW:0:9}, 无新提交"
fi

# ---- hold when lineage differs or diverges a lot ----------------------------
HOLD=""
[ -z "$BASE" ] && HOLD="新分支和 $TARGET 没有共同祖先"
[ -z "$HOLD" ] && [ "$NFOREIGN" -gt 0 ] && HOLD="$TARGET 里有 $NFOREIGN 个 Windows 线提交不在 \`$SEL_NAME\` 里 (不同谱系, 或 Windows 重写了历史)"
[ -z "$HOLD" ] && [ "$AHEAD" -gt "$MAX_AHEAD" ] && HOLD="新分支比 $TARGET 多 $AHEAD 个提交 (> $MAX_AHEAD, 大幅分叉)"
if [ -n "$HOLD" ] && [ $FORCE = 0 ]; then
    report_commits
    R ""; R "## 🟠 没有自动合并"; R ""; R "原因: $HOLD"; R ""
    if [ "$NFOREIGN" -gt 0 ]; then R "$TARGET 里有、新分支里没有的 Windows 线提交 (前 10):"; R '```'; head -n 10 "$FOREIGN" | while read -r l; do R "$l"; done; R '```'; fi
    predict_conflicts
    if [ "$PRED_RC" = 1 ]; then R "如果硬合并, 预计冲突的文件:"; for f in $PRED_FILES; do R "$(patch_info "$f")"; done
    elif [ "$PRED_RC" = 0 ]; then R "试合并 (merge-tree) 没有冲突。"; fi
    R ""; R "要接受请加 \`--force-switch\` 再运行; 想换分支请用 \`--branch NAME\`。状态文件没有更新。"
    bad "HELD: $HOLD"
    say "   merge-base $BASE9; rerun with --force-switch to accept, or --branch NAME"
    finish 3 HELD "$HOLD"
fi
[ -n "$HOLD" ] && warn "--force-switch: merging despite: $HOLD"

report_commits

# ---- dry run ----------------------------------------------------------------
if [ $DRY = 1 ]; then
    predict_conflicts
    R ""; R "## 冲突预测 (git merge-tree, 不改工作区)"; R ""
    case "$PRED_RC" in
        0) R "无冲突。"; say "dry-run: trial merge is clean ($AHEAD new commits)" ;;
        1) bad "dry-run: trial merge would CONFLICT:"; R "预计冲突 (真正合并会 abort 并报告):"
           for f in $PRED_FILES; do say "   $f"; R "$(patch_info "$f")"; done ;;
        *) R "(这个 git 不支持 merge-tree --write-tree, 无法预测)" ;;
    esac
    [ $PUSH_FORK = 1 ] && R "" && R "--push-fork: dry-run 不推送。"
    finish 0 DRYRUN "dry-run: $SEL_NAME@${NEW:0:9}, 新增 $AHEAD 个提交, 没有合并$([ "$PRED_RC" = 1 ] && echo ', 预计有冲突')"
fi

# ---- merge ------------------------------------------------------------------
if [ -z "$(g config user.name)" ]; then   # no identity configured: use the team's
    export GIT_AUTHOR_NAME="${GIT_AUTHOR_NAME:-PigBun-AI}" GIT_AUTHOR_EMAIL="${GIT_AUTHOR_EMAIL:-261591371+PigBun-AI@users.noreply.github.com}"
    export GIT_COMMITTER_NAME="${GIT_COMMITTER_NAME:-$GIT_AUTHOR_NAME}" GIT_COMMITTER_EMAIL="${GIT_COMMITTER_EMAIL:-$GIT_AUTHOR_EMAIL}"
fi
MSG="mac: merge $SEL_NAME (${NEW:0:9}) into $TARGET"
say "== merge $SEL_NAME@${NEW:0:9} into $TARGET"
MOUT="$(mktmp)"
if [ $UPTODATE = 1 ]; then
    NEW_HEAD="$OLD_HEAD"; say "nothing to merge (--check-anyway)"
elif g merge --no-ff --no-edit -m "$MSG" "$NEW" > "$MOUT" 2>&1; then
    NEW_HEAD="$(g rev-parse HEAD)"
    say "merged: ${OLD_HEAD:0:9} -> ${NEW_HEAD:0:9}"
else
    if [ -e "$(g rev-parse --path-format=absolute --git-path MERGE_HEAD)" ]; then
        CFILES="$(g diff --name-only --diff-filter=U)"
        g merge --abort
        R ""; R "## 🔴 冲突 (已 \`merge --abort\`, 工作区回到 \`${OLD_HEAD:0:9}\`)"; R ""
        bad "MERGE CONFLICT in:"
        for f in $CFILES; do say "   $f"; R "$(patch_info "$f")"; done
        [ "$(g rev-parse HEAD)" = "$OLD_HEAD" ] && [ -z "$(g status --porcelain --untracked-files=no)" ] \
            && { R ""; R "已核对: HEAD 仍是 \`${OLD_HEAD:0:9}\`, 工作区干净。"; say "aborted; tree is back at ${OLD_HEAD:0:9}"; } \
            || bad "WARNING: tree is not back to the old state, check $WT by hand"
        finish 2 CONFLICT "合并 $SEL_NAME@${NEW:0:9} 冲突, 已 abort ($(printf '%s\n' "$CFILES" | grep -c .) 个文件)"
    fi
    sed 's/^/  /' "$MOUT" >&2
    R ""; R "## 🔴 merge 失败 (不是内容冲突)"; R ""; R '```'; R "$(cat "$MOUT")"; R '```'
    finish 1 ERROR "git merge 失败: $(head -n 1 "$MOUT")"
fi
R ""; R "## 合并"; R ""
if [ $UPTODATE = 1 ]; then R "- 没有新提交, HEAD 仍是 \`${OLD_HEAD:0:9}\` (只做验证)"
else R "- \`${OLD_HEAD:0:9}\` → \`${NEW_HEAD:0:9}\` (merge 提交, 保留 \`mac:\` 提交历史)"; fi
R "- 冲突: 无"
R "- 本次合并的文件改动: $(g diff --shortstat "$OLD_HEAD" "$NEW_HEAD")"
printf 'last_branch=%s\nlast_commit=%s\nlast_head=%s\nlast_run=%s\n' "$SEL_NAME" "$NEW" "$NEW_HEAD" "$NOW_H" > "$STATE_FILE"

# ---- build + test -----------------------------------------------------------
NB="-"; NT="-"; NC="-"; FAILED_TESTS=""; CHECK_OK=1
if [ $NOCHECK = 1 ]; then
    R ""; R "## 验证"; R ""; R "(--no-check: 跳过)"; warn "checks skipped (--no-check)"
else
    LOGS="$WT/target/mac-logs"; mkdir -p "$LOGS"; rm -f "$LOGS"/native-build.log "$LOGS"/native-test.log "$LOGS"/cross-build.log
    say "== tools/mac/check.sh (native build, native tests, cross build); logs $LOGS"
    T0="$(date +%s)"
    TO=""; command -v timeout >/dev/null 2>&1 && TO="timeout -k 30 $CHECK_TIMEOUT"
    if [ -f "$WT/tools/mac/check.sh" ]; then
        # shellcheck disable=SC2086
        ( cd "$WT" && LOGS="$LOGS" $TO sh tools/mac/check.sh ) > "$LOGS/check.out" 2>&1; CHECK_RC=$?
    else
        # equivalent of check.sh for trees that do not have it
        ( cd "$WT" && . tools/mac/env.sh && \
          { cargo build --workspace --locked > "$LOGS/native-build.log" 2>&1; \
            cargo test --workspace --locked --no-fail-fast > "$LOGS/native-test.log" 2>&1; \
            cargo build --release -p victory --target x86_64-pc-windows-gnullvm --locked > "$LOGS/cross-build.log" 2>&1; \
            ls -l target/x86_64-pc-windows-gnullvm/release/victory.exe; } ) > "$LOGS/check.out" 2>&1; CHECK_RC=$?
    fi
    ELAPSED=$(( $(date +%s) - T0 ))
    nb="$LOGS/native-build.log"; nt="$LOGS/native-test.log"; nc="$LOGS/cross-build.log"
    NB=FAIL; [ -f "$nb" ] && grep -Eq '^[[:space:]]*Finished ' "$nb" && ! grep -Eq '^error' "$nb" && NB=PASS
    FAILED_TESTS="$([ -f "$nt" ] && grep -E '^test .* \.\.\. FAILED$' "$nt" | sed -E 's/^test (.*) \.\.\. FAILED$/\1/' | sort -u)"
    NT=FAIL
    if [ -f "$nt" ] && [ -z "$FAILED_TESTS" ] && ! grep -Eq '^error|^test result: FAILED' "$nt" && grep -q '^test result: ok' "$nt"; then NT=PASS; fi
    [ $NB = FAIL ] && [ "$NT" = PASS ] && NT=FAIL
    NPASS="$([ -f "$nt" ] && awk '/^test result: /{for(i=1;i<NF;i++) if($(i+1)=="passed;") s+=$i} END{print s+0}' "$nt")"
    NC=FAIL; [ -f "$nc" ] && grep -Eq '^[[:space:]]*Finished ' "$nc" && ! grep -Eq '^error' "$nc" \
        && [ -f "$WT/target/x86_64-pc-windows-gnullvm/release/victory.exe" ] && NC=PASS
    [ $CHECK_RC -ne 0 ] && [ "$NB$NT$NC" = PASSPASSPASS ] && NC=FAIL
    [ "$NB$NT$NC" = PASSPASSPASS ] && CHECK_OK=1 || CHECK_OK=0
    say "build: $NB   tests: $NT ($NPASS passed)   cross build: $NC   (${ELAPSED}s)"
    [ -n "$FAILED_TESTS" ] && { bad "failed tests:"; printf '%s\n' "$FAILED_TESTS" | sed 's/^/   /'; }
    R ""; R "## 验证 (tools/mac/check.sh, ${ELAPSED}s)"; R ""
    ICON() { [ "$1" = PASS ] && echo "✅ PASS" || echo "🔴 **FAIL**"; }
    R "| 步骤 | 结果 |"; R "|---|---|"
    R "| 原生 build (cargo build --workspace) | $(ICON $NB) |"
    R "| 原生 test (cargo test --workspace, 通过 $NPASS 个) | $(ICON $NT) |"
    R "| 交叉编译 victory.exe (gnullvm release) | $(ICON $NC) |"
    R ""; R "日志: \`$LOGS/\` (native-build.log, native-test.log, cross-build.log, check.out)"
    if [ -n "$FAILED_TESTS" ]; then R ""; R "### 🔴 失败的测试"; R ""; printf '%s\n' "$FAILED_TESTS" | sed 's/^/- `/; s/$/`/' >> "$BODY"; fi
    if [ $CHECK_OK = 0 ]; then
        R ""; R "### 🔴 错误摘录"; R ""; R '```'
        for lg in "$nb" "$nc"; do
            [ -f "$lg" ] && grep -E -A8 -m2 '^error' "$lg" | head -n 24 | while IFS= read -r l; do R "$l"; done
        done
        if [ -f "$nt" ]; then   # failing tests: their output, then cargo's summary of failed targets
            { grep -E -A6 '^---- .* stdout ----$' "$nt" | head -n 40; grep -E -A6 '^error(\[|: [0-9]+ targets? failed|: could not compile)' "$nt" | head -n 20; } | while IFS= read -r l; do R "$l"; done
        fi
        R '```'
        bad "CHECKS FAILED (merge kept: $TARGET is at ${NEW_HEAD:0:9})"
    fi
fi

# ---- rollback / push ----------------------------------------------------------
FINAL_RC=0; FINAL_STATUS=OK
FINAL_HEAD="$NEW_HEAD"
if [ $CHECK_OK = 0 ]; then
    FINAL_RC=4; FINAL_STATUS=CHECKFAIL
    if [ $ROLLBACK = 1 ]; then
        if [ "$(g rev-parse HEAD)" = "$NEW_HEAD" ] && [ -z "$(g status --porcelain --untracked-files=no)" ]; then
            g reset --hard "$OLD_HEAD" >/dev/null && { R ""; R "--rollback-on-fail: $TARGET 已回到 \`${OLD_HEAD:0:9}\`。"; warn "rolled back to ${OLD_HEAD:0:9}"; FINAL_HEAD="$OLD_HEAD"
                printf 'last_branch=%s\nlast_commit=%s\nlast_head=%s\nlast_run=%s\n' "${LAST_BRANCH:-$SEL_NAME}" "${LAST_COMMIT:-}" "$OLD_HEAD" "$NOW_H" > "$STATE_FILE"; }
        else
            R ""; R "--rollback-on-fail 没有执行: $TARGET 的 HEAD 已不是这次的合并提交或工作区有改动 (有人在期间提交了?)。"; bad "rollback skipped: HEAD moved or tree dirty"
        fi
    else
        R ""; R "合并已保留 (没有加 \`--rollback-on-fail\`)。"
    fi
fi
if [ $PUSH_FORK = 1 ]; then
    R ""; R "## 推送到 fork"; R ""
    FURL="$(g remote get-url "$FORK_REMOTE" 2>/dev/null || true)"
    if [ $CHECK_OK = 0 ]; then R "验证失败, 不推送。"; say "push skipped: checks failed"
    elif ! printf '%s' "$FURL" | grep -q 'PigBun-AI/VIctory-macOS'; then R "remote \`$FORK_REMOTE\` 是 \`${FURL:-?}\`, 不是 PigBun-AI/VIctory-macOS: 不推送。"; bad "push refused: $FORK_REMOTE -> ${FURL:-?}"; FINAL_RC=1; FINAL_STATUS=ERROR
    elif g push "$FORK_REMOTE" "refs/heads/$TARGET:refs/heads/$TARGET" > "$MOUT" 2>&1; then R "已推送 \`$TARGET\` → \`$FORK_REMOTE\` (普通 push, 非 force)。"; say "pushed $TARGET to $FORK_REMOTE"
    else R "推送失败:"; R '```'; R "$(cat "$MOUT")"; R '```'; bad "push to $FORK_REMOTE failed"; FINAL_RC=1; FINAL_STATUS=ERROR; fi
fi

SUM="$SEL_NAME@${NEW:0:9} 合入 $TARGET (${OLD_HEAD:0:9} → ${NEW_HEAD:0:9}), 新增 $AHEAD 个提交"
[ "$FINAL_HEAD" = "$OLD_HEAD" ] && [ "$NEW_HEAD" != "$OLD_HEAD" ] && SUM="$SUM, 已回滚到 ${OLD_HEAD:0:9}"
case "$FINAL_STATUS" in
    OK) [ $NOCHECK = 1 ] && SUM="$SUM; 未验证 (--no-check)" || SUM="$SUM; build/test/交叉编译全部通过" ;;
    CHECKFAIL) SUM="$SUM; 🔴 验证失败: build=$NB test=$NT cross=$NC${FAILED_TESTS:+ 失败测试: $(printf '%s' "$FAILED_TESTS" | head -n 3 | tr '\n' ' ')}" ;;
esac
[ $SWITCHED = 1 ] && SUM="🔴 分支变了 ($LAST_BRANCH → $SEL_NAME)! $SUM"
finish "$FINAL_RC" "$FINAL_STATUS" "$SUM"
