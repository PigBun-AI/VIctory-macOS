#!/usr/bin/env bash
# tools/mac/push-check.sh -- check what is about to be pushed to the public `fork` remote.
#
#   tools/mac/push-check.sh [BRANCH] [options]        (usage: docs/mac/PUSH.md)
#
# The range is `git rev-list BRANCH --not --remotes=fork` (BRANCH defaults to HEAD): every commit
# the fork does not have yet. Checks:
#   (a) `cargo xtask policy`
#   (b) credentials in added lines (PASSWORD/TOKEN/SECRET/askpass, private-key headers, ghp_,
#       github_pat_, AKIA, ...). Hits are printed with literal values masked, for a human to judge.
#   (c) private information: RFC 1918 addresses, /Users/<name>/ and C:\Users\ paths, Windows drive
#       project paths, *.local e-mail addresses, words from the private word list, and `mac:`
#       commits whose author/committer is not the placeholder identity.
#   (d) new binaries, blobs over 1 MB, game-file extensions.
#
# The private word list (real name, user name, host names, ...) is NOT in the repository: it is read
# from tools/mac/privacy.local.txt (untracked, in .gitignore), else $PRIVACY_WORDS_FILE, else
# <git common dir>/privacy.local.txt. One word per line; `w:` in front = whole word only. Values of
# PIGBUN_CRED_*_USER / _USERNAME / _HOST in the environment are added when present.
#
# Exit code: 0 clean (review items may still be printed), 1 something blocks the push,
# 2 the check could not run (bad usage, no word list, not a git repo).
# Findings are never printed with the private word or the secret value itself.

set -u
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT" || exit 2

BRANCH=""; REMOTE=fork; SKIP_POLICY=0; EXTRA=()
while [ $# -gt 0 ]; do
    case "$1" in
        -h|--help) sed -n '2,/^# Findings are never/p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        --remote) REMOTE="${2:?}"; shift 2 ;;
        --skip-policy) SKIP_POLICY=1; shift ;;
        --words) export PRIVACY_WORDS_FILE="${2:?}"; shift 2 ;;
        --windows-review|--strict|--no-words|-v|--verbose) EXTRA+=("$1"); shift ;;
        -*) echo "push-check: unknown option: $1 (try --help)" >&2; exit 2 ;;
        *) [ -z "$BRANCH" ] && BRANCH="$1" || { echo "push-check: one branch only" >&2; exit 2; }; shift ;;
    esac
done
[ -n "$BRANCH" ] || BRANCH=HEAD
git rev-parse --verify -q "$BRANCH^{commit}" >/dev/null || { echo "push-check: no such branch/commit: $BRANCH" >&2; exit 2; }
COMMON="$(git rev-parse --path-format=absolute --git-common-dir)"

RC=0
echo "== push-check: $BRANCH ($(git rev-parse --short=9 "$BRANCH")) against remotes/$REMOTE/*"

# ---- (a) policy ------------------------------------------------------------
if [ "$SKIP_POLICY" = 1 ]; then
    echo "== (a) cargo xtask policy: SKIPPED (--skip-policy)"
else
    echo "== (a) cargo xtask policy"
    if [ "$(git rev-parse HEAD)" != "$(git rev-parse "$BRANCH^{commit}")" ]; then
        echo "   note: policy checks the checked-out tree (HEAD), which is not $BRANCH"
    fi
    if (
        # the toolchain comes from tools/mac/env.sh when this checkout has it (tools/metal/env.sh is not needed, and is never read here)
        if [ -f "$ROOT/tools/mac/env.sh" ]; then
            . "$ROOT/tools/mac/env.sh" >/dev/null 2>&1 || echo "   note: tools/mac/env.sh found no toolchain (set VICTORY_MAC_TOOLCHAIN); using the cargo on PATH"
        else
            echo "   note: no tools/mac/env.sh in this checkout; using the cargo on PATH"
        fi
        command -v cargo >/dev/null 2>&1 || { echo "   cargo not found (tools/mac/env.sh did not help)"; exit 2; }
        # building xtask is a heavy job: through heavy.sh (docs/mac/RUN.md "Heavy tasks") where this checkout has it
        _hv=""; [ -f "$ROOT/tools/mac/heavy.sh" ] && _hv="bash $ROOT/tools/mac/heavy.sh --jobs 1 --label push-check-policy --"
        $_hv cargo xtask policy 2>&1 | sed 's/^/   /'
        exit "${PIPESTATUS[0]}"
    ); then echo "   policy: ok"; else echo "   BLOCK (a) cargo xtask policy failed"; RC=1; fi
fi

# ---- (b) (c) (d) over the range ----------------------------------------------
export PC_BRANCH="$BRANCH" PC_REMOTE="$REMOTE" PC_COMMON="$COMMON" PC_ROOT="$ROOT"
python3 - ${EXTRA[@]+"${EXTRA[@]}"} <<'PY'
import os, re, subprocess, sys

args = sys.argv[1:]
WINDOWS_REVIEW = '--windows-review' in args
STRICT = '--strict' in args
NO_WORDS = '--no-words' in args
branch, remote, common, root = (os.environ[k] for k in ('PC_BRANCH', 'PC_REMOTE', 'PC_COMMON', 'PC_ROOT'))
IDENT = ('PigBun-AI', '261591371+PigBun-AI@users.noreply.github.com')


def git(*a, **kw):
    p = subprocess.run(('git',) + a, cwd=root, stdout=subprocess.PIPE, stderr=subprocess.PIPE, **kw)
    if p.returncode != 0:
        sys.stderr.write(p.stderr.decode('utf-8', 'replace'))
        sys.exit(2)
    return p.stdout


rng = git('rev-list', '--reverse', branch, '--not', '--remotes=' + remote).decode().split()
print('== range: %d commit(s) not on %s' % (len(rng), remote))
if not rng:
    print('   nothing to push')
    sys.exit(0)
if len(rng) > 500:
    print('   WARN: very large range; is remote %s fetched?' % remote)

# ---- private word list --------------------------------------------------------
words = []  # (regex, label)
source = None
if not NO_WORDS:
    cands = [os.path.join(root, 'tools/mac/privacy.local.txt'), os.environ.get('PRIVACY_WORDS_FILE', ''),
             os.path.join(common, 'privacy.local.txt')]
    for c in cands:
        if c and os.path.isfile(c):
            source = c
            break
    if source is None:
        print('   BLOCK (c) no private word list: put one at tools/mac/privacy.local.txt '
              '(or set PRIVACY_WORDS_FILE); --no-words skips this check')
        sys.exit(2)
    raw = [l.rstrip('\n') for l in open(source, encoding='utf-8')]
    vals = [l.strip() for l in raw if l.strip() and not l.lstrip().startswith('#')]
    for k, v in os.environ.items():
        if re.match(r'^PIGBUN_CRED_.*_(USER|USERNAME|HOST)$', k) and v.strip():
            vals.append(v.strip())
    seen = set()
    for v in vals:
        whole = v.startswith('w:')
        w = v[2:] if whole else v
        if len(w) < 3 or w.lower() in seen:
            continue
        seen.add(w.lower())
        pat = re.escape(w)
        if whole:
            pat = r'(?<![A-Za-z0-9_])' + pat + r'(?![A-Za-z0-9_])'
        words.append(re.compile(pat, re.I))
    print('== private word list: %d entries (%s)' % (len(words), 'tools/mac/privacy.local.txt' if source == cands[0] else 'other location'))

# ---- commit metadata ------------------------------------------------------------
meta = {}
out = git('log', '--no-walk=unsorted', '--stdin', '-z', '--format=%H%x1f%an%x1f%ae%x1f%cn%x1f%ce%x1f%B',
          input=('\n'.join(rng) + '\n').encode()).decode('utf-8', 'replace')
for rec in out.split('\0'):
    rec = rec.strip('\n')
    if not rec:
        continue
    h, an, ae, cn, ce, body = rec.split('\x1f', 5)
    meta[h] = dict(an=an, ae=ae, cn=cn, ce=ce, body=body, subj=body.split('\n', 1)[0])

blocks = {'b': 0, 'c': 0, 'd': 0}
reviews = {'b': 0, 'c': 0, 'd': 0}


def is_mac(h):
    return meta[h]['subj'].startswith('mac:')


def report(cat, h, where, what, text=None, review=False):
    if WINDOWS_REVIEW and not is_mac(h):
        review = True
    if STRICT:
        review = False
    (reviews if review else blocks)[cat] += 1
    tag = 'REVIEW' if review else 'BLOCK'
    line = '   %s (%s) %s %s: %s' % (tag, cat, h[:8], where, what)
    if text is not None:
        line += '\n       | ' + text[:200]
    print(line)


# ---- patterns ---------------------------------------------------------------------
TOKEN_RE = re.compile(r'(ghp_|gho_|ghu_|ghs_|ghr_|github_pat_|glpat-|xox[abprs]-|sk-)[A-Za-z0-9_\-]{10,}|AKIA[0-9A-Z]{16}|AIza[0-9A-Za-z_\-]{30,}')
PRIVKEY_RE = re.compile(r'-----BEGIN [A-Z0-9 ]*PRIVATE KEY( BLOCK)?-----')
KEYWORD_RE = re.compile(r'passw(or)?d|passwd|token|secret|askpass|api[_-]?key', re.I)
LITERAL_RE = re.compile(r'''(?i)(passw(?:or)?d|passwd|token|secret|api[_-]?key)\w*["']?\s*[:=]\s*["']([^"'$\s{}<>`]{8,})["']''')
LONGRUN_RE = re.compile(r'[A-Za-z0-9+/=_\-]{24,}')


def redact(line):
    line = TOKEN_RE.sub('<redacted-token>', line)
    line = PRIVKEY_RE.sub('<private-key-header>', line)
    line = LITERAL_RE.sub(lambda m: m.group(0).replace(m.group(2), '<redacted>'), line)
    def longrun(m):
        s = m.group(0)
        if re.fullmatch(r'[A-Za-z0-9_]+', s) and '_' in s:
            return s  # an identifier or variable name, not a value
        if re.search(r'[A-Za-z]', s) and re.search(r'\d', s):
            return '<redacted?>'
        return s
    line = LONGRUN_RE.sub(longrun, line)
    return line.strip()


GENERIC_USERS = r'(?:Shared|Public|Guest|Default|user|users|username|name|you|me|foo|bar|example|runner|admin|someone|USER|USERNAME|NAME|YOU|YOURNAME|yourname|your-name|your_name|<[^/\\]*>|\$[^/\\]*|%[^/\\]*%|\{[^/\\]*\}|\.\.\.?|~)'
PRIV_PATTERNS = [
    ('192.168/16 address', re.compile(r'(?<![\d.])192\.168\.\d{1,3}\.\d{1,3}(?![\d])')),
    ('10/8 address', re.compile(r'(?<![\d.])10\.\d{1,3}\.\d{1,3}\.\d{1,3}(?![\d])')),
    ('172.16/12 address', re.compile(r'(?<![\d.])172\.(?:1[6-9]|2\d|3[01])\.\d{1,3}\.\d{1,3}(?![\d])')),
    ('/Users/<name>/ path', re.compile(r'/Use[r]s/(?!' + GENERIC_USERS + r'(?:/|$|[\s"\'`)]))[^/\s"\'`<>$]+')),
    ('C:\\Users\\<name> path', re.compile(r'(?i)[A-Z]:[\\/]{1,2}Users[\\/]{1,2}(?!' + GENERIC_USERS + r'(?:[\\/]|$|[\s"\'`)]))[^\\/\s"\'`<>$%]+')),
    ('Windows drive project path', re.compile(r'(?i)(?<![A-Za-z0-9])[A-Z]:[\\/]{1,2}(?:project|projects|GTA6|work|dev|src|code|repo|repos|git|Documents|Desktop)(?![A-Za-z0-9_])')),
    ('*.local e-mail address', re.compile(r'[\w.+\-]+@[\w\-]+(?:\.[\w\-]+)*\.local\b', re.I)),
]


def scan_privacy(h, where, text):
    n = 0
    for label, rx in PRIV_PATTERNS:
        if rx.search(text):
            report('c', h, where, label, rx.sub('<match>', text).strip())
            n += 1
    if words:
        hit = text
        found = False
        for w in words:
            if w.search(hit):
                hit = w.sub('<private-word>', hit)
                found = True
        if found:
            report('c', h, where, 'private word list', hit.strip())
            n += 1
    return n


# ---- (c) metadata and messages ---------------------------------------------------------
print('== (c) commit identities and messages')
for h in rng:
    m = meta[h]
    if is_mac(h):
        for who, n, e in (('author', m['an'], m['ae']), ('committer', m['cn'], m['ce'])):
            if (n, e) != IDENT:
                report('c', h, 'commit', 'mac: commit %s is not PigBun-AI <noreply> (name/e-mail not the placeholder)' % who)
    for i, ln in enumerate(m['body'].split('\n'), 1):
        scan_privacy(h, 'message:%d' % i, ln)
        if TOKEN_RE.search(ln) or PRIVKEY_RE.search(ln):
            report('b', h, 'message:%d' % i, 'secret-looking text', redact(ln))
print('   done')

# ---- added lines (b) (c) ---------------------------------------------------------------
print('== (b) credentials and (c) private information in added lines')
diff = git('diff-tree', '--stdin', '-p', '-U0', '-r', '--root', '--no-renames', '--no-color', '--text',
           input=('\n'.join(rng) + '\n').encode()).decode('utf-8', 'replace')
cur = None
path = None
lineno = 0
for ln in diff.split('\n'):
    if re.match(r'^[0-9a-f]{40}$', ln):
        cur = ln
        continue
    if cur is None:
        continue
    if ln.startswith('diff --git '):
        path = None
        continue
    if ln.startswith('+++ '):
        path = ln[4:]
        path = path[2:] if path.startswith('b/') else path
        # file names are scanned too
        if path != '/dev/null':
            scan_privacy(cur, 'path', path)
        continue
    if ln.startswith('--- '):
        continue
    m = re.match(r'^@@ -\S+ \+(\d+)', ln)
    if m:
        lineno = int(m.group(1))
        continue
    if ln.startswith('+'):
        text = ln[1:]
        where = '%s:%d' % (path, lineno)
        lineno += 1
        if TOKEN_RE.search(text):
            report('b', cur, where, 'token-looking string', redact(text))
        elif PRIVKEY_RE.search(text):
            report('b', cur, where, 'private key header', redact(text))
        elif LITERAL_RE.search(text):
            report('b', cur, where, 'literal secret-looking assignment', redact(text))
        elif KEYWORD_RE.search(text):
            report('b', cur, where, 'keyword for review', redact(text), review=True)
        scan_privacy(cur, where, text)

# ---- (d) binaries, big blobs, game files ------------------------------------------------------
print('== (d) binaries, blobs over 1 MB, game-file extensions')
GAME_EXT = {'rpf', 'ytd', 'ydr', 'yft', 'ydd', 'ymap', 'ytyp', 'ybn', 'ycd', 'ymt', 'awc', 'xvd', 'xvc', 'msixvc', 'xbf',
            'bik', 'bk2', 'bnk', 'wem', 'cdxil', 'dxbc', 'pak', 'bank', 'gfx'}
raw = git('diff-tree', '--stdin', '-r', '--root', '--no-renames', '--raw', '--abbrev=40',
          input=('\n'.join(rng) + '\n').encode()).decode('utf-8', 'replace')
num = git('diff-tree', '--stdin', '-r', '--root', '--no-renames', '--numstat', '--format=%H',
          input=('\n'.join(rng) + '\n').encode()).decode('utf-8', 'replace')
binary = set()
cur = None
for ln in num.split('\n'):
    if re.match(r'^[0-9a-f]{40}$', ln):
        cur = ln
    elif ln.startswith('-\t-\t') and cur:
        binary.add((cur, ln.split('\t', 2)[2]))
cur = None
sizes = {}
for ln in raw.split('\n'):
    if re.match(r'^[0-9a-f]{40}$', ln):
        cur = ln
        continue
    m = re.match(r'^:(\d+) (\d+) ([0-9a-f]+) ([0-9a-f]+) ([AMT])\t(.*)$', ln)
    if not m or not cur:
        continue
    mode, sha, path = m.group(2), m.group(4), m.group(6)
    if mode == '160000':
        continue
    ext = path.rsplit('.', 1)[-1].lower() if '.' in os.path.basename(path) else ''
    if ext in GAME_EXT:
        report('d', cur, path, 'game-file extension .%s' % ext)
    if (cur, path) in binary:
        report('d', cur, path, 'binary file added/changed')
    if sha not in sizes:
        sizes[sha] = int(git('cat-file', '-s', sha).decode().strip())
    if sizes[sha] > 1024 * 1024:
        report('d', cur, path, 'blob of %.1f MB' % (sizes[sha] / 1048576.0))

# ---- summary ----------------------------------------------------------------------------------
print('== summary: BLOCK b=%d c=%d d=%d   REVIEW b=%d c=%d d=%d' % (
    blocks['b'], blocks['c'], blocks['d'], reviews['b'], reviews['c'], reviews['d']))
sys.exit(1 if sum(blocks.values()) else 0)
PY
PYRC=$?
[ "$PYRC" -eq 2 ] && exit 2
[ "$PYRC" -ne 0 ] && RC=1
if [ "$RC" -eq 0 ]; then echo "== push-check: CLEAN"; else echo "== push-check: BLOCKED (exit 1)"; fi
exit "$RC"
