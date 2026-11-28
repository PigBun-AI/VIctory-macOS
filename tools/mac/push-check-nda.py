#!/usr/bin/env python3
"""nda-gdkx: the push-check class that keeps the NDA SDK (GDKX) headers out of the public fork. Path rules only.

Called by tools/mac/push-check.sh (section (e), also alone as `--nda-only`) and by the pre-push hook (tools/mac/install-hooks.sh).
Every hit is a BLOCK: no REVIEW level, no waiver, no flag turns it off.

  push-check-nda.py [--root DIR] range REV-LIST-ARGS...    the files of the commits selected by `git rev-list ARGS`

A file the range adds or changes is refused when
  * its name is a header (.h/.hpp) of the NDA SDK: d3d12_x*, d3d12x*, xg*, XGame*, *_xs, pix3_xbox, dstorage_xs
    (d3d12_x.dll, xg.rs, xgame.rs are fine: only header extensions are matched);
  * a directory of its path contains gdkx or gxdk;
  * a directory of its path is called private.
Hits are printed as commit, path and the rule. Nothing else is looked at.
"""
import fnmatch
import os
import subprocess
import sys

CLASS = 'nda-gdkx'
FILE_PATTERNS = ('d3d12_x*.h', 'd3d12_x*.hpp', 'd3d12x*.h', 'd3d12x*.hpp', 'xg*.h', 'xg*.hpp',
                 'xgame*.h', 'xgame*.hpp', '*_xs.h', '*_xs.hpp', 'pix3_xbox.h', 'dstorage_xs.h')
DIR_SUBSTRINGS = ('gdkx', 'gxdk')


def path_reasons(path):
    segs = path.replace('\\', '/').split('/')
    base = segs[-1].lower()
    dirs = [s.lower() for s in segs[:-1]]
    out = []
    for pat in FILE_PATTERNS:
        if fnmatch.fnmatchcase(base, pat):
            out.append('header file name matches %s' % pat)
            break
    if any(x in d for d in dirs for x in DIR_SUBSTRINGS):
        out.append('directory name contains gdkx/gxdk')
    if 'private' in dirs:
        out.append('private/ path segment')
    return out


def main(argv):
    root = os.getcwd()
    a = argv[:]
    if len(a) >= 2 and a[0] == '--root':
        root, a = a[1], a[2:]
    if len(a) < 2 or a[0] != 'range':
        print('usage: push-check-nda.py [--root DIR] range REV-LIST-ARGS...', file=sys.stderr)
        return 2

    def git(*args, input=None):
        p = subprocess.run(('git', '-c', 'core.quotepath=false') + args, cwd=root, input=input,
                           stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        if p.returncode != 0:
            sys.stderr.write(p.stderr.decode('utf-8', 'replace'))
            sys.exit(2)
        return p.stdout.decode('utf-8', 'replace')

    print('== (e) %s: NDA SDK header names, gdkx/gxdk and private/ paths (any hit blocks; no review level, no waiver)' % CLASS)
    rng = git('rev-list', '--reverse', *a[1:]).split()
    print('   range: %d commit(s)' % len(rng))
    blocks = 0
    if rng:
        out = git('diff-tree', '--stdin', '-r', '--root', '--no-renames', '--name-status', '--format=%H',
                  input=('\n'.join(rng) + '\n').encode())
        cur = None
        for ln in out.split('\n'):
            if len(ln) == 40 and all(c in '0123456789abcdef' for c in ln):
                cur = ln
            elif '\t' in ln and cur:
                st, pth = ln.split('\t', 1)
                if st.startswith('D'):
                    continue
                for r in path_reasons(pth):
                    blocks += 1
                    print('   BLOCK (%s) %s %s: %s' % (CLASS, cur[:8], pth, r))
    if blocks:
        print('   %s: BLOCK (%d)' % (CLASS, blocks))
        return 1
    print('   %s: clean' % CLASS)
    return 0


if __name__ == '__main__':
    sys.exit(main(sys.argv[1:]))
