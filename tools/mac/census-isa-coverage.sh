#!/bin/sh
# How much of the title's instruction-form census (census-title-mnemonics.sh) the Rosetta probes (tools/mac/rosetta-probe,
# rosetta-probe-wine.sh) have actually proven. Static presence only: whether the title EXECUTES a form is unknown for all of them
# (used=unknown; the census has no execution profile). Three tiers per form:
#   proven    a probe case checks this form's result (bit-exact against softref.h) or its fault/atomic behaviour under Wine
#   partial   same mnemonic or class probed, this exact form not (e.g. a register move where only the store was probed)
#   unproven  no probe of the mnemonic
# usage: tools/mac/census-isa-coverage.sh [census-dir] [outdir]       (default: the newest research/census/title-mnemonics-*; outdir = the same)
# Writes coverage.tsv (one row per category) and coverage.txt (totals by instructions, forms, mnemonics) and prints the totals.
set -eu
root=$HOME/Documents/GTA6Project
dir=${1:-$(ls -d "$root"/research/census/title-mnemonics-* | sort | tail -1)}
out=${2:-$dir}
[ -f "$dir/forms.tsv" ] || { echo "no $dir/forms.tsv (run tools/mac/census-title-mnemonics.sh)" >&2; exit 1; }
python3 - "$dir" "$out" <<'PYTHON_END'
import csv, re, sys, collections
d, out = sys.argv[1], sys.argv[2]
BMI = set('andn bextr blsi blsmsk blsr bzhi pdep pext mulx rorx sarx shlx shrx tzcnt lzcnt popcnt'.split())
FMA = re.compile(r'^vf(n?m)(add|sub)(132|213|231)(ps|pd|ss|sd)$|^vfm(addsub|subadd)(132|213|231)(ps|pd)$')
CTRL = set('call jmp ret retf loop'.split())
PAD = {'int3', 'nop'}
ALU = set('add sub and or xor cmp test inc dec neg not adc sbb'.split())
MOVE = set('mov movzx movsx movsxd movabs lea push pop xchg cdqe cdq cqo cwde cwd pushfq addr32 xlatb'.split())
SHIFT = set('shl shr sar rol ror imul mul div idiv bt bts btr btc bsr bsf bswap movbe shld shrd'.split())
FENCE = {'mfence', 'sfence', 'lfence', 'pause'}
SYS = set('cpuid xgetbv clwb clzero monitorx mwaitx insertq in out cli hlt ud2 int enter <unknown> insb'.split())
STR = set('stosw stosq stosb stosd scasb scasd movsb movsw movsd movsq cmpsb'.split())

def is_jcc(m): return re.fullmatch(r'j(a|ae|b|be|e|g|ge|l|le|ne|no|np|ns|o|p|s|z|nz|c|nc|cxz|rcxz)', m) is not None
def is_cmov(m): return m.startswith('cmov')
def is_set(m): return re.fullmatch(r'set[a-z]+', m) is not None

def category(m):
    if m in PAD: return 'padding (int3, nop)'
    if m in BMI: return 'bmi1/bmi2/lzcnt/popcnt'
    if FMA.match(m): return 'fma3'
    if m in ('vcvtph2ps', 'vcvtps2ph'): return 'f16c'
    if m in ('cmpxchg', 'cmpxchg16b', 'xadd'): return 'atomic rmw (cmpxchg, xadd)'
    if m in ('adcx', 'adox'): return 'adx (adcx, adox)'
    if 'aes' in m or 'clmul' in m: return 'aes / pclmulqdq'
    if m in FENCE: return 'fences, pause (tso)'
    if m in ('cpuid', 'xgetbv'): return 'cpuid, xgetbv'
    if m in ('clwb', 'clzero', 'monitorx', 'mwaitx', 'insertq'): return 'amd-only / cache write-back'
    if m.startswith('prefetch'): return 'prefetch'
    if m in SYS: return 'system / rare (in, out, cli, hlt, ud2, int, enter, unknown bytes)'
    if m in STR: return 'string ops'
    if m == 'fdiv' or m.startswith('f') and not m.startswith('fma'): return 'x87'
    if m in CTRL or is_jcc(m): return 'integer: control flow (call, jmp, jcc, ret)'
    if m in MOVE or is_cmov(m) or is_set(m): return 'integer: move, lea, push/pop, cmov, setcc'
    if m in ALU: return 'integer: add, sub, logic, cmp, test'
    if m in SHIFT: return 'integer: shift, rotate, mul, div, bit ops'
    if m.startswith('v'):
        if 'maskmov' in m: return 'avx: masked load/store'
        if m in ('vzeroupper', 'vzeroall', 'vstmxcsr', 'vldmxcsr'): return 'avx: vzeroupper, mxcsr'
        if m.startswith('vmovnt'): return 'nontemporal stores'
        if re.match(r'^v(mov|broadcast|pbroadcast|pmov)', m) or m in ('vpextrb','vpextrw','vpextrd','vpextrq','vextractps','vpinsrb','vpinsrw','vpinsrd','vpinsrq','vinsertps'):
            return 'avx: moves, broadcast, extract/insert, sign/zero-extend'
        if re.search(r'(shuf|perm|blend|unpck|alignr|insert|extract|pack|palign)', m): return 'avx: shuffle, permute, blend, pack'
        if m.startswith('vcvt'): return 'avx: conversion (int/float, ps/pd)'
        if m.startswith('vp'): return 'avx: integer simd (vpadd, vpcmp, vpand, vpsll ...)'
        return 'avx: float arithmetic, compare, logic, rcp/rsqrt/round'
    if m.startswith('movnt'): return 'nontemporal stores'
    if m in ('movdqu','movdqa','movups','movaps','movd','movq','movss','movsd','movapd','movupd'): return 'sse legacy: moves'
    return 'sse legacy: arithmetic, logic, shuffle, convert'

def tier(form, m):
    """proven / partial / unproven for one form (see the head of this file)."""
    first = form.split(' ', 1)[1].split(',')[0] if ' ' in form else ''
    store = first.startswith('m')            # destination is memory (Intel syntax)
    lock = form.startswith('lock ')
    if m in BMI or FMA.match(m) or m in ('vcvtph2ps', 'vcvtps2ph'): return 'proven'
    if m in ('cmpxchg', 'cmpxchg16b', 'xadd') or m == 'xchg' and store: return 'proven'
    if lock and m in ('or', 'bts'): return 'proven'
    if lock: return 'partial'                # lock and/add/sub/xor/inc/dec/btr/btc: class probed through or/bts/xadd
    if m in ('cpuid', 'xgetbv', 'clwb'): return 'proven'
    if m in ('prefetcht0', 'prefetchw'): return 'proven'
    if m.startswith('prefetch'): return 'partial'
    if m.startswith('vmovnt') or m.startswith('movnt'): return 'proven'
    if 'maskmov' in m: return 'proven' if store else 'partial'     # stores probed; masked loads only share the mask logic
    if m in ('vextractf128', 'vextracti128'): return 'proven' if store else 'partial'
    if m in ('vmovups','vmovaps','vmovdqu','vmovdqa','movups','movaps','movdqu','movdqa'): return 'proven' if store and ('x' in first or first in ('m128','m256')) else 'partial'
    if m == 'mov': return 'proven' if store and form.split(',')[-1] in ('r32', 'r64') else 'partial'
    if m in ('stosq', 'stosw', 'stosb', 'stosd', 'movsb', 'movsq'): return 'proven' if form.startswith('rep') else 'partial'
    if m in ('clwb',): return 'proven'
    return 'unproven'

PROBE = {
 'bmi1/bmi2/lzcnt/popcnt': 'batch bmi: 46 forms x 60,000 sets, bit-exact vs softref.h (arm64-validated)',
 'fma3': 'batch fma/fma_nan/fma_ftz: 60 forms, random + boundary + 4 roundings + FTZ/DAZ + NaN rules, bit-exact',
 'f16c': 'batch f16c: all 65,536 halves for vcvtph2ps; vcvtps2ph imm 0-4 x 4 roundings x 3.2M; memory form: fault cases (fatal, patched by L1)',
 'atomic rmw (cmpxchg, xadd)': 'probe-wine fault matrix + atomic_flags/atomic_split batches (hit/miss, data, flags, access type, 322 offsets)',
 'avx: masked load/store': 'probe-wine masked store cases x 8 modes x masks (stores); masked loads not probed',
 'nontemporal stores': 'probe-wine movnti, vmovnt*, movnt* fault-resume',
 'cpuid, xgetbv': 'probe-wine cpuid / xgetbv',
 'amd-only / cache write-back': 'clwb probed (illegal instruction); clzero, monitorx, mwaitx, insertq: loader/console_insn unit tests only, not Rosetta-probed',
 'prefetch': 'probe-wine prefetcht0, prefetchw (never fault)',
 'string ops': 'probe-wine rep stosq, rep movsb fault-resume',
 'integer: move, lea, push/pop, cmov, setcc': 'only mov r32/r64 stores (fault-resume); everything else unprobed',
 'avx: moves, broadcast, extract/insert, sign/zero-extend': 'vmovups/vmovdqu/vmovaps/vmovdqa stores and vextract128 to memory (fault-resume)',
 'sse legacy: moves': 'movups/movdqu store fault-resume',
}
cats = collections.OrderedDict()
tot = collections.Counter(); forms_n = collections.Counter(); mn_best = {}
for r in csv.DictReader(open(f'{d}/forms.tsv'), delimiter='\t'):
    f, m, n = r['form'], r['mnemonic'], int(r['count'])
    c = category(m); t = tier(f, m)
    e = cats.setdefault(c, {'mn': set(), 'forms': 0, 'n': 0, 'proven': 0, 'partial': 0, 'unproven': 0, 'fproven': 0, 'fpartial': 0, 'funproven': 0})
    e['mn'].add(m); e['forms'] += 1; e['n'] += n; e[t] += n; e['f' + t] += 1
    tot['n'] += n; tot['forms'] += 1; tot[t] += n; forms_n[t] += 1
    if c == 'padding (int3, nop)': tot['pad'] += n
    rank = {'proven': 2, 'partial': 1, 'unproven': 0}
    mn_best.setdefault(m, []).append(t)
mnt = collections.Counter()
for m, ts in mn_best.items():
    mnt['proven' if all(x == 'proven' for x in ts) else 'partial' if any(x != 'unproven' for x in ts) else 'unproven'] += 1
rows = sorted(cats.items(), key=lambda kv: -kv[1]['n'])
with open(f'{out}/coverage.tsv', 'w') as fh:
    fh.write('category\tmnemonics\tforms\tinstructions\tshare_pct\tproven\tpartial\tunproven\tforms_proven\tforms_partial\tforms_unproven\tprobe_evidence\n')
    for c, e in rows:
        fh.write('%s\t%d\t%d\t%d\t%.2f\t%d\t%d\t%d\t%d\t%d\t%d\t%s\n' % (c, len(e['mn']), e['forms'], e['n'], 100.0 * e['n'] / tot['n'],
            e['proven'], e['partial'], e['unproven'], e['fproven'], e['fpartial'], e['funproven'], PROBE.get(c, 'none')))
nopad = tot['n'] - tot['pad']
lines = [
 f"title instruction census: {tot['n']} instructions, {tot['forms']} forms, {len(mn_best)} mnemonics (static presence; execution unknown: used=unknown)",
 f"by instructions (all): proven {tot['proven']} ({100.0*tot['proven']/tot['n']:.1f}%), partial {tot['partial']} ({100.0*tot['partial']/tot['n']:.1f}%), unproven {tot['unproven']} ({100.0*tot['unproven']/tot['n']:.1f}%)",
 f"by instructions (without int3/nop padding, {nopad}): proven {100.0*tot['proven']/nopad:.1f}%, partial {100.0*tot['partial']/nopad:.1f}%, unproven {100.0*tot['unproven']/nopad:.1f}%",
 f"by forms: proven {forms_n['proven']} ({100.0*forms_n['proven']/tot['forms']:.1f}%), partial {forms_n['partial']}, unproven {forms_n['unproven']} ({100.0*forms_n['unproven']/tot['forms']:.1f}%)",
 f"by mnemonics: proven {mnt['proven']} (every form), partial {mnt['partial']} (some form probed), unproven {mnt['unproven']} ({100.0*mnt['unproven']/len(mn_best):.1f}%)",
]
open(f'{out}/coverage.txt', 'w').write('\n'.join(lines) + '\n')
print('\n'.join(lines)); print()
for c, e in rows:
    print('%-62s %9d  forms %4d  proven %9d partial %9d unproven %9d' % (c[:62], e['n'], e['forms'], e['proven'], e['partial'], e['unproven']))
PYTHON_END
