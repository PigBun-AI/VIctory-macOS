#!/usr/bin/env python3
"""Who reads the flags a `lock cmpxchg` leaves (matrix row mac.isa.lock_cmpxchg_flags, docs/mac/PATCHES.md).

Rosetta computes ZF of a `cmpxchg` right but CF, SF, OF, PF (and AF) of a miss from `memory - accumulator`
instead of `accumulator - memory` (probe: 114,699 misses, 100% reversed). That is harmless wherever
nothing reads those flags before they are overwritten. This walks the code after every `lock cmpxchg`
with a memory operand, through fall-through and direct branches, and says for each site what happens first:

  safe       every path overwrites CF/SF/OF/PF (cmp, test, add, ...) or ends (call, ret, indirect jmp,
             int3, ud2) before any instruction reads one of them; only ZF readers (je, jne, sete, cmove, ...)
             were met on the way
  reader     some path reaches a reader of CF/SF/OF/PF while one of those flags is still the cmpxchg's:
             ja/jb/jl/jg/js/jo/jp.., seta../setb.., cmova.., adc, sbb, rcl, rcr, cmc, lahf, pushf
  unresolved the walk gave up (more than STEPS instructions on one path, or a direct branch out of
             the decoded range)

`--selftest` runs the walk over a few made-up listings (a reader, an overwritten flag, a loop, ...)
and exits 0 when each is classified as it should be.

Input: `llvm-objdump -d --x86-asm-syntax=intel -j .text <image>` on stdin; output directory argv[1].
Writes cmpxchg-flags-summary.txt, cmpxchg-flags-readers.tsv (every reader site: rva, mnemonic of the
reader, its rva, the path length), cmpxchg-flags-unresolved.tsv. Prints the summary.

The decoded range is the whole .text (about 26.7 million instructions, kept as three compact arrays).
Rules (conservative where it matters): a flag counts as overwritten only by an instruction that surely
writes it (cmp, test, add, sub, and, or, xor, neg, mul, imul, ucomiss, popcnt, ... all five; inc/dec all but
CF; shifts and rotates only by an immediate; bt* CF); flags an instruction leaves undefined count as
overwritten, since no code can depend on them. A call, ret, indirect jmp, int3 and ud2 end a path with the
flags dead (the compiler does not keep them across those). AF is not tracked (only lahf/pushf read it).
"""
import sys, re, os, collections, bisect
from array import array

BASE = 0x140000000
STEPS = 400

line_re = re.compile(r'^\s*([0-9a-f]+):\s+(?:[0-9a-f]{2} )+\s*\t(.*)$')
PREFIXES = {'lock', 'rep', 'repz', 'repnz', 'repe', 'repne', 'notrack', 'bnd', 'data16', 'cs', 'ds', 'es', 'ss',
            'fs', 'gs', 'xacquire', 'xrelease', 'addr32', 'rex', 'rex.w', 'rex.b', 'rex.x', 'rex.r'}

# flags: C=1 S=2 O=4 P=8 (AF is not read by anything but lahf/pushf and daa-like opcodes of 32-bit code)
C, S, O, P = 1, 2, 4, 8
ALL = C | S | O | P

# which of C S O P (Z is not tracked: cmpxchg's ZF is right) each condition code reads
COND = {
    'o': O, 'no': O, 'b': C, 'c': C, 'nae': C, 'ae': C, 'nb': C, 'nc': C, 'e': 0, 'z': 0, 'ne': 0, 'nz': 0,
    'be': C, 'na': C, 'a': C, 'nbe': C, 's': S, 'ns': S, 'p': P, 'pe': P, 'np': P, 'po': P,
    'l': S | O, 'nge': S | O, 'ge': S | O, 'nl': S | O, 'le': S | O, 'ng': S | O, 'g': S | O, 'nle': S | O,
}

# what an instruction surely writes (so the cmpxchg's value of that flag is gone)
WRITES = {}
for m in ('add', 'sub', 'cmp', 'test', 'and', 'or', 'xor', 'neg', 'xadd', 'cmpxchg', 'popcnt', 'ucomiss', 'ucomisd',
          'comiss', 'comisd', 'vucomiss', 'vucomisd', 'vcomiss', 'vcomisd', 'ptest', 'vptest', 'vtestps',
          'vtestpd', 'testps', 'testpd', 'sahf', 'popfq', 'popf', 'fcomi', 'fucomi', 'fcomip', 'fucomip',
          'mul', 'imul', 'andn', 'blsr', 'blsi', 'blsmsk', 'bextr', 'bzhi', 'lzcnt', 'tzcnt', 'bsf', 'bsr',
          'adc', 'sbb', 'xrstor', 'xrstors', 'rdrand', 'rdseed'):
    WRITES[m] = ALL
WRITES['inc'] = WRITES['dec'] = O | S | P
WRITES['clc'] = WRITES['stc'] = C
for m in ('bt', 'bts', 'btr', 'btc'):
    WRITES[m] = C
for m in ('shl', 'shr', 'sar', 'sal', 'shld', 'shrd'):
    WRITES[m] = ALL  # only with an immediate count (see walk)
for m in ('rol', 'ror'):
    WRITES[m] = C | O  # only with an immediate count
SHIFTS = ('shl', 'shr', 'sar', 'sal', 'rol', 'ror', 'shld', 'shrd')

READS_C = {'adc', 'sbb', 'rcl', 'rcr', 'cmc', 'adcx'}
READS_ALL = {'lahf', 'pushf', 'pushfq', 'pushfw'}
READS_O = {'adox'}

mnames = {}
mlist = []
addr = mn_id = aux = kind = None
sites = []
n = 0


def mid(m):
    i = mnames.get(m)
    if i is None:
        i = len(mlist)
        mnames[m] = i
        mlist.append(m)
    return i


def cond_of(m):
    for pre in ('cmov', 'set', 'j'):
        if m.startswith(pre):
            c = m[len(pre):]
            if c in COND:
                return COND[c]
    return None


def classify(m):
    """('end',) | ('jmp',) | ('jcc', mask) | ('reads', mask) | ('writes', mask) | ('plain',)"""
    if m in ('ret', 'retq', 'int3', 'ud2', 'hlt', 'iret', 'iretq', 'syscall', 'sysret'):
        return ('end',)
    if m == 'jmp':
        return ('jmp',)
    if m == 'call':
        return ('end',)  # flags are not preserved across a call (callee-clobbered)
    if m in ('jrcxz', 'jecxz', 'loop', 'loope', 'loopne', 'loopz', 'loopnz'):
        return ('jcc', 0)
    if m.startswith('j') and cond_of(m) is not None:
        return ('jcc', cond_of(m))
    if (m.startswith('cmov') or m.startswith('set')) and cond_of(m) is not None:
        return ('reads', cond_of(m))
    if m in READS_C:
        return ('reads', C)
    if m in READS_O:
        return ('reads', O)
    if m in READS_ALL:
        return ('reads', ALL)
    if m in WRITES:
        return ('writes', WRITES[m])
    return ('plain',)


def load(lines):
    """Fills addr, mn_id, aux, kind, sites from `llvm-objdump -d --x86-asm-syntax=intel` lines."""
    global addr, mn_id, aux, kind, sites, n
    addr = array('Q')
    mn_id = array('H')
    aux = array('Q')  # a direct branch's target; or 1 for a shift/rotate by an immediate
    sites = []        # indexes of lock cmpxchg [mem]
    pending_lock = False
    for line in lines:
        m = line_re.match(line)
        if not m:
            continue
        a = int(m.group(1), 16)
        txt = m.group(2).split('#')[0].strip()
        toks = txt.split(None, 1)
        if not toks:
            continue
        locked = pending_lock
        pending_lock = False
        while toks and toks[0] in PREFIXES:
            locked = locked or toks[0] == 'lock'
            txt = toks[1] if len(toks) > 1 else ''
            toks = txt.split(None, 1)
        if not toks:
            # objdump prints a prefix it could not attach (`lock` before a byte-sized cmpxchg) on a line of
            # its own; it is an instruction address a branch can name
            pending_lock = locked
            addr.append(a)
            mn_id.append(mid('(prefix)'))
            aux.append(0)
            continue
        mn = toks[0]
        ops = toks[1].strip() if len(toks) > 1 else ''
        x = 0
        if mn.startswith('j') or mn == 'call':
            t = ops.split()[0] if ops else ''
            if re.fullmatch(r'0x[0-9a-f]+|[0-9a-f]+', t):
                x = int(t, 16)
        elif mn in SHIFTS:
            last = ops.rsplit(',', 1)[-1].strip()
            if re.fullmatch(r'(0x)?[0-9a-f]+', last) and not last.startswith('cl'):
                x = 1
        if mn == 'cmpxchg' and locked and '[' in ops:
            sites.append(len(addr))
        addr.append(a)
        mn_id.append(mid(mn))
        aux.append(x)
    n = len(addr)
    kind = [classify(m) for m in mlist]
    sys.stderr.write(f'decoded {n} instructions, {len(sites)} lock cmpxchg [mem] sites\n')


def index_of(a):
    i = bisect.bisect_left(addr, a)
    return i if i < n and addr[i] == a else None


def walk(start):
    """-> ('safe',) | ('reader', mnemonic, rva, steps) | ('unresolved', why)"""
    seen = set()
    stack = [(start, ALL, 0)]
    unresolved = None
    while stack:
        i, live, steps = stack.pop()
        while live:
            if i >= n:
                unresolved = unresolved or 'ran off the text'
                break
            if (i, live) in seen:
                break
            seen.add((i, live))
            steps += 1
            if steps > STEPS:
                unresolved = unresolved or f'more than {STEPS} instructions'
                break
            k = kind[mn_id[i]]
            t = k[0]
            if t == 'end':
                break
            if t == 'reads':
                if k[1] & live:
                    return ('reader', mlist[mn_id[i]], addr[i] - BASE, steps)
            elif t == 'jcc':
                if k[1] & live:
                    return ('reader', mlist[mn_id[i]], addr[i] - BASE, steps)
                tgt = index_of(aux[i]) if aux[i] else None
                if tgt is None:
                    unresolved = unresolved or f'branch to {aux[i]:#x} out of the decoded instructions'
                else:
                    stack.append((tgt, live, steps))
            elif t == 'jmp':
                if aux[i]:
                    tgt = index_of(aux[i])
                    if tgt is None:
                        unresolved = unresolved or f'jmp to {aux[i]:#x} out of the decoded instructions'
                        break
                    i = tgt
                    continue
                break  # indirect: the compiler does not keep flags across it
            elif t == 'writes':
                w = k[1]
                if mlist[mn_id[i]] in SHIFTS and aux[i] != 1:
                    w = 0  # by cl: no flag is written when the count is 0
                live &= ~w
            i += 1
    if unresolved:
        return ('unresolved', unresolved)
    return ('safe',)


def run(lines, outdir=None):
    load(lines)
    res = collections.Counter()
    readers, unres = [], []
    by_reader = collections.Counter()
    for si in sites:
        r = walk(si + 1)
        res[r[0]] += 1
        if r[0] == 'reader':
            readers.append((addr[si] - BASE, r[1], r[2], r[3]))
            by_reader[r[1]] += 1
        elif r[0] == 'unresolved':
            unres.append((addr[si] - BASE, r[1]))
    summary = [
        f'lock cmpxchg [mem] sites {len(sites)}',
        f'  safe        {res["safe"]}',
        f'  reader      {res["reader"]}',
        f'  unresolved  {res["unresolved"]}',
        'readers by mnemonic: ' + ', '.join(f'{m} {c}' for m, c in by_reader.most_common()),
        f'walk limit {STEPS} instructions per path; text of {n} instructions',
    ]
    if outdir:
        os.makedirs(outdir, exist_ok=True)
        with open(os.path.join(outdir, 'cmpxchg-flags-readers.tsv'), 'w') as f:
            f.write('site_rva\treader\treader_rva\tinstructions_after_cmpxchg\n')
            for s, m, r, st in readers:
                f.write(f'{s:#x}\t{m}\t{r:#x}\t{st}\n')
        with open(os.path.join(outdir, 'cmpxchg-flags-unresolved.tsv'), 'w') as f:
            f.write('site_rva\twhy\n')
            for s, why in unres:
                f.write(f'{s:#x}\t{why}\n')
        open(os.path.join(outdir, 'cmpxchg-flags-summary.txt'), 'w').write('\n'.join(summary) + '\n')
    return res, summary


def listing(*insns):
    """A made-up objdump listing: each item 'mnemonic operands' or ('mnemonic operands', target-label)."""
    out, a = [], BASE + 0x1000
    for text in insns:
        out.append(f'{a:x}: 90                          \t{text}')
        a += 4
    return out


def selftest():
    lc = 'lock cmpxchg dword ptr [rdi], esi'

    def verdict(*insns):
        run(listing(lc, *insns))
        return walk(sites[0] + 1)[0]

    def at(k):  # address of the k-th instruction of the listing
        return f'{BASE + 0x1000 + 4 * k:#x}'

    cases = [
        ('a ZF branch, then an overwrite, then an unsigned compare', 'safe',
         ['jne ' + at(4), 'cmp rax, rbx', 'ja ' + at(4), 'ret']),
        ('an unsigned branch right after', 'reader', ['ja ' + at(9)]),
        ('a signed compare reads SF/OF', 'reader', ['mov eax, 1', 'jl ' + at(9)]),
        ('setb', 'reader', ['setb al']),
        ('sete is ZF only', 'safe', ['sete al', 'ret']),
        ('adc reads CF', 'reader', ['adc rax, 0']),
        ('overwritten first (test)', 'safe', ['test eax, eax', 'jl ' + at(9), 'ret']),
        ('inc keeps CF: a CF reader after it still sees cmpxchg\'s', 'reader', ['inc eax', 'jb ' + at(9)]),
        ('a call ends the path', 'safe', ['call ' + at(9), 'ja ' + at(9)]),
        ('pushfq reads them all', 'reader', ['pushfq']),
        ('a shift by cl may not write flags', 'reader', ['shl eax, cl', 'ja ' + at(9)]),
        ('a shift by an immediate does', 'safe', ['shl eax, 3', 'ja ' + at(9), 'ret']),
        ('a loop back to the cmpxchg, which writes them all', 'safe', ['jne ' + at(0), 'ret']),
        ('a reader on the taken side of a ZF branch', 'reader', ['je ' + at(4), 'ret', 'ret', 'jb ' + at(4)]),
        ('a direct jmp is followed', 'reader', ['jmp ' + at(4), 'ret', 'ret', 'cmovg eax, ebx']),
        ('an indirect jmp ends the path', 'safe', ['jmp rax', 'ja ' + at(9)]),
        ('a branch out of the listing is not guessed', 'unresolved', ['je 0x140999999', 'ret']),
    ]
    bad = 0
    for name, want, insns in cases:
        got = verdict(*insns)
        if got != want:
            bad += 1
            print(f'FAIL {name}: want {want}, got {got}')
    # the byte-sized cmpxchg: `lock` on a line of its own, and a branch to it
    run(listing('jne ' + at(2), 'nop', 'lock', 'cmpxchg byte ptr [rdi], sil', 'ja ' + at(9)))
    if len(sites) != 1 or walk(sites[0] + 1)[0] != 'reader':
        bad += 1
        print('FAIL: lock on a line of its own')
    print(f'{len(cases) + 1 - bad} of {len(cases) + 1} selftest cases pass')
    return bad


if __name__ == '__main__':
    if len(sys.argv) > 1 and sys.argv[1] == '--selftest':
        sys.exit(1 if selftest() else 0)
    res, summary = run(sys.stdin, sys.argv[1])
    print('\n'.join(summary))
