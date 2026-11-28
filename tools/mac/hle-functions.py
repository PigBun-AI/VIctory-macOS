#!/usr/bin/env python3
"""Per-function HLE status of the title's import table (static, no game run, no Windows needed).

usage: tools/mac/hle-functions.py [import-callsites.tsv] [out.csv.txt] [--summary]
  import-callsites.tsv  default research/census/title-isa-20261127/import-callsites.tsv (written by tools/mac/census-title-imports.sh;
                        columns dll, function, callsites; a leading "(address taken) " on the dll marks calls through a stored pointer)
  out.csv.txt           default docs/mac/hle-functions.csv.txt (in the repo: names and counts only)
  --summary             also print the per-module table

Resolution rule, copied from crates/victory-hle/src/lib.rs HleResolver::resolve and apps/victory/src/run.rs (`wanted`):
  1. an export of a module in crates/victory-hle/src/modules (hle_export!(X), HleExport { name: "X" .. }, ordinal("#N", ..)) whose
     `dlls` list serves the DLL                                      -> implemented, or stub (see below)
  2. the DLL is allowed by HostPolicy::default() (host.rs)             -> passthrough-to-host (Wine's own DLL answers it)
  3. a module serves the DLL but has no such export                   -> missing (the loader binds a reporting stub: UnimplementedPolicy)
  4. neither served nor allowed                                       -> title-dll (shipped by the title, mapped by the loader: not ours)
Passthrough functions are checked against the export table of the Wine DLL in target/wine-cx/lib/wine/x86_64-windows (PE export directory, forwarders
count as exported): not exported -> `missing`; DLL file not found there -> stays passthrough-to-host with that said in `basis`. Whether the HLE function is a stub is a
heuristic on its source (the doc comment above says `shortcut:`/`stub`/`not implemented`, or the body is at most 6 lines and calls nothing). The `basis` column says which. Call frequency is NOT here: it needs the run-time counters (mac.runtime.hle_counters).
The mac module (SuspendThread H1) is included: it is only registered under Rosetta, which is the only case that matters here.
"""
import csv, os, re, sys, collections

here = os.path.dirname(os.path.abspath(__file__))
repo = os.path.dirname(os.path.dirname(here))
args = [a for a in sys.argv[1:] if not a.startswith('--')]
imports = args[0] if args else os.path.expanduser('~/Documents/GTA6Project/research/census/title-isa-20261127/import-callsites.tsv')
out = args[1] if len(args) > 1 else os.path.join(repo, 'docs/mac/hle-functions.csv.txt')
modules_dir = os.path.join(repo, 'crates/victory-hle/src/modules')
wine_dir = os.environ.get('WINE_DLL_DIR') or os.path.expanduser('~/Documents/GTA6Project/VIctory-macOS/target/wine-cx/lib/wine/x86_64-windows')
import struct

_exports = {}
def wine_exports(dll):
    """set of exported names (and '#ordinal') of Wine's builtin <dll>, or None if the file is not there."""
    key = dll.lower()
    if key in _exports: return _exports[key]
    # Wine has no file per API set: ntdll's schema sends api-ms-win-core-* to kernelbase/kernel32/ntdll, api-ms-win-crt-* to ucrtbase.
    if key.startswith('api-ms-win-crt-') or key.startswith('api-ms-win-core-') or key.startswith('ext-ms-win-rtcore-ntuser-'):
        cands = ['ucrtbase.dll'] if '-crt-' in key else ['user32.dll'] if 'ntuser' in key else ['kernelbase.dll', 'kernel32.dll', 'ntdll.dll', 'combase.dll', 'advapi32.dll']
        sets = [wine_exports(c) for c in cands]
        res = None if all(x is None for x in sets) else set().union(*[x for x in sets if x])
        _exports[key] = res
        return res
    path = None
    if os.path.isdir(wine_dir):
        for f in os.listdir(wine_dir):
            if f.lower() == key or f.lower() == key + '.dll' and not key.endswith('.dll'): path = os.path.join(wine_dir, f); break
    res = None
    if path:
        d = open(path, 'rb').read()
        pe = struct.unpack_from('<I', d, 0x3c)[0]
        nsec = struct.unpack_from('<H', d, pe + 6)[0]; opt = struct.unpack_from('<H', d, pe + 20)[0]
        magic = struct.unpack_from('<H', d, pe + 24)[0]
        dd = pe + 24 + (112 if magic == 0x20b else 96)
        rva, size = struct.unpack_from('<II', d, dd)
        secs = [struct.unpack_from('<IIII', d, pe + 24 + opt + 40 * i + 8) for i in range(nsec)]
        def off(r):
            for vs, va, rs, raw in secs:
                if va <= r < va + max(vs, rs): return raw + r - va
            return None
        res = set()
        if rva:
            e = off(rva)
            nnames, base_ord = struct.unpack_from('<I', d, e + 24)[0], struct.unpack_from('<I', d, e + 16)[0]
            nfun = struct.unpack_from('<I', d, e + 20)[0]
            names_rva, ords_rva = struct.unpack_from('<II', d, e + 32)
            for i in range(nnames):
                nr = struct.unpack_from('<I', d, off(names_rva) + 4 * i)[0]
                o = off(nr); res.add(d[o:d.index(b'\0', o)].decode('ascii', 'replace'))
            for i in range(nfun): res.add('#%d' % (base_ord + i))
    _exports[key] = res
    return res
host_rs = os.path.join(repo, 'crates/victory-hle/src/host.rs')

def serves(patterns, dll):
    dll = dll.lower()
    return any(dll.startswith(p[:-1]) if p.endswith('*') else dll == p for p in patterns)

# HostPolicy::default allow list
src = open(host_rs, encoding='utf-8').read()
body = src[src.index('impl Default for HostPolicy'):]
body = body[:body.index('];')]
allow = re.findall(r'^\s*"([^"]+)",', body, re.M)

STUB_WORDS = re.compile(r'shortcut:|\bstub\b|not implemented|E_NOTIMPL', re.I)
QUIET = re.compile(r'(set_last_error|SetLastError|tracing::\w+!?|log::\w+!?|println!|eprintln!|debug!|warn!|info!|once)\s*\(?')
def split_fn(text, name):
    """(comment lines above, body text) of `fn name` in a Rust source, or None."""
    m = re.search(r'(?m)^[ \t]*(?:pub(?:\([a-z]+\))?\s+)?(?:unsafe\s+)?(?:extern\s+"[a-z0-9]+"\s+)?fn\s+%s\b' % re.escape(name), text)
    if not m: return None
    above = []
    for line in reversed(text[:m.start()].split('\n')[:-1]):
        s = line.strip()
        if s.startswith('//'): above.append(s)
        elif s.startswith('#[') or s == '': 
            if s == '' and above: break
        else: break
    i = text.index('{', m.end()); depth = 0; j = i
    while j < len(text):
        c = text[j]
        if c == '{': depth += 1
        elif c == '}':
            depth -= 1
            if depth == 0: break
        j += 1
    return ' '.join(above), text[i + 1:j]

def classify(text, rust_name):
    r = split_fn(text, rust_name)
    if r is None: return 'implemented', 'fn not found in the module file (defined elsewhere): treated as implemented'
    doc, body = r
    code = [l.strip() for l in body.split('\n') if l.strip() and not l.strip().startswith('//')]
    if STUB_WORDS.search(doc): return 'stub', 'doc comment says stub/shortcut/not implemented'
    inner = ' '.join(l.strip() for l in body.split('\n') if l.strip().startswith('//'))
    if re.search(r'shortcut:', inner): return 'stub', 'a comment in the body says shortcut:'
    rest = QUIET.sub('', ' '.join(code))
    if len(code) <= 6 and not re.search(r'[A-Za-z_][A-Za-z_0-9:]*\s*\(', rest):
        return 'stub', 'body of %d code lines that calls nothing (returns a constant)' % len(code)
    return 'implemented', 'body of %d code lines' % len(code)

registry = []   # (module, dlls, {export name: (status, basis)})
for fn in sorted(os.listdir(modules_dir)):
    if not fn.endswith('.rs'): continue
    text = open(os.path.join(modules_dir, fn), encoding='utf-8').read()
    for mm in re.finditer(r'fn module\(\)\s*->\s*HleModule\s*\{', text):
        j = mm.end(); depth = 1
        while depth:
            c = text[j]; depth += (c == '{') - (c == '}'); j += 1
        mod = text[mm.start():j]
        mname = re.search(r'HleModule\s*\{\s*name:\s*"([^"]+)"', mod).group(1)
        dm = re.search(r'dlls:\s*&\[(.*?)\]', mod, re.S)
        dlls = re.findall(r'"([^"]+)"', dm.group(1)) if dm else []
        ex = {}
        for e in re.finditer(r'hle_export!\((\w+)\)', mod): ex[e.group(1)] = classify(text, e.group(1))
        for e in re.finditer(r'HleExport\s*\{\s*name:\s*"([^"]+)"\s*,\s*address:\s*([^}]+?)\s*,?\s*\}', mod):
            f = re.findall(r'\b(\w+)\s+as\s+\*const', e.group(2))
            ex[e.group(1)] = classify(text, f[0]) if f else ('implemented', 'data export')
        for e in re.finditer(r'ordinal\("(#\d+)",\s*(\w+)\s+as', mod): ex[e.group(1)] = classify(text, e.group(2))
        registry.append((fn[:-3], dlls, ex))   # module label = source file (registry names are free text: xsystem is called "device type")

rows = collections.OrderedDict()
for r in csv.reader(open(imports, encoding='utf-8'), delimiter='\t'):
    if r[0] == 'dll': continue
    taken = r[0].startswith('(address taken) ')
    dll = r[0].replace('(address taken) ', '')
    k = (dll.lower(), r[1]); e = rows.setdefault(k, {'dll': dll, 'fn': r[1], 'direct': 0, 'taken': 0})
    e['taken' if taken else 'direct'] += int(r[2])

result = []
for (dl, f), e in rows.items():
    status = basis = module = None
    for mname, dlls, ex in registry:
        if serves(dlls, dl) and f in ex:
            status, basis = ex[f]; module = mname
            if serves(allow, dl): basis += '; overrides the host'
            break
    if status is None:
        servers = [m for m, dlls, ex in registry if serves(dlls, dl)]
        if serves(allow, dl):
            ex = wine_exports(dl)
            if ex is None: status, module, basis = 'passthrough-to-host', 'host', 'DLL allowed by HostPolicy; Wine DLL file not found in %s: export not checked' % os.path.basename(wine_dir.rstrip('/'))
            elif f in ex: status, module, basis = 'passthrough-to-host', 'host', 'DLL allowed by HostPolicy; exported by Wine'
            else: status, module, basis = 'missing', 'host', 'DLL allowed by HostPolicy but Wine\'s DLL does not export it: the loader binds a reporting stub'
        elif servers: status, module, basis = 'missing', servers[0], 'module serves the DLL but has no such export: unimplemented-import stub'
        else: status, module, basis = 'title-dll', 'title', 'DLL shipped by the title, mapped by the loader'
    result.append((e['dll'], f, e['direct'], e['taken'], status, module, basis))

os.makedirs(os.path.dirname(out), exist_ok=True)
with open(out, 'w', encoding='utf-8', newline='') as fh:
    w = csv.writer(fh, lineterminator='\n')
    w.writerow(['dll', 'function', 'static_callsites', 'address_taken_sites', 'status', 'module', 'basis', 'runtime_calls'])
    for r in sorted(result, key=lambda r: (r[4], r[5], -(r[2] + r[3]), r[0].lower(), r[1])):
        w.writerow(list(r) + [''])
cnt = collections.Counter(r[4] for r in result)
print('functions %d (unique dll+name): %s' % (len(result), dict(cnt)), '->', out)
if '--summary' in sys.argv:
    by = collections.defaultdict(collections.Counter); sites = collections.Counter()
    for r in result:
        by[r[5]][r[4]] += 1; sites[r[5]] += r[2] + r[3]
    for m, c in sorted(by.items(), key=lambda kv: -sites[kv[0]]):
        print('%-12s sites %5d  %s' % (m, sites[m], dict(c)))
