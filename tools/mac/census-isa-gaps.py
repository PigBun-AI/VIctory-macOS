#!/usr/bin/env python3
"""Impact of the two ISA translation gaps found by the isa2 probes, measured on the title image.

  gap 1  vpsllvq / vpsrlvq: Rosetta shifts by the LOW 32 BITS of each 64-bit count lane. A lane whose high 32 bits are
         not zero and whose low 32 bits are <= 63 gives a wrong result (Intel: count > 63 gives 0). Everything else
         (high half zero, or low half >= 64) is right.
  gap 2  vptest / vtestps / vtestpd: PF and AF are left as they were instead of being cleared. ZF, CF, SF, OF are right.

gap 1 (count origin): for every site the count operand (the last operand) is traced backwards through straight code and
over direct branches (all predecessors of a join) to the instruction that defined it, and an abstract value is computed:
    ('ub', B)    every lane is <= B (unsigned)
    ('mask',)    every lane is 0 or 2^64-1 (a compare result)
    None         unknown (a subtraction, a negation, an unknown load, a function argument, ...)
  safe      B < 2^32 (high half always zero), or a compare mask (0 or all ones: low half is then 0 or >= 64)
  suspect   unknown value, or the trace did not finish (more than STEPS instructions)
Constants are read from the image (rip-relative loads: the `# 0xADDR` comment of the disassembly). A call clobbers
xmm0-xmm5; xmm6-xmm15 survive it. The entry of a function (no predecessor) makes the value unknown.

gap 2 (flag readers): the walk of census-cmpxchg-flags.py, started with PF live only (AF is read only by lahf/pushf,
which count as readers of everything): `reader` = some path reads PF/AF before it is overwritten.

`--selftest` runs both analyses on made-up listings, each with a deliberately constructed reader / suspect site that
the tool must report, and safe cases it must not report (the analysis-side negative control).

Input: a disassembly file from `llvm-objdump -d --x86-asm-syntax=intel -j .text --no-show-raw-insn <image>` (the raw-bytes
form is accepted too), the image itself (constants), an output directory. Output is game-derived: keep it outside the repo.
usage: census-isa-gaps.py <disasm.txt> <image.dll> <outdir>   |   census-isa-gaps.py --selftest
"""
import sys, re, os, mmap, struct, bisect, collections, importlib.util
from array import array

HERE = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location('cf', os.path.join(HERE, 'census-cmpxchg-flags.py'))
cf = importlib.util.module_from_spec(spec)
spec.loader.exec_module(cf)

BASE = 0x140000000
STEPS = 30000
M64 = (1 << 64) - 1
sys.setrecursionlimit(20000)

# ------------------------------------------------------------------ disassembly
class Dis:
    """addr[i], mnemonic id, direct-branch target address (0 if none), file offset of the line; operands are parsed on demand."""
    def __init__(self):
        self.addr = array('Q'); self.mn = array('H'); self.tgt = array('Q'); self.off = array('Q')
        self.names = {}; self.mlist = []; self.f = None; self.buf = None; self.sites = collections.defaultdict(list)
        self.targets = None; self.preds = None; self.isbt = None

    def mid(self, m):
        i = self.names.get(m)
        if i is None:
            i = self.names[m] = len(self.mlist); self.mlist.append(m)
        return i

    def load(self, lines, offsets=None):
        pending = None
        for k, line in enumerate(lines):
            if not line or line[0] == ' ' and ':' not in line[:20]:
                pass
            p = line.split('\t')
            if len(p) < 2 or ':' not in p[0]:
                continue
            try:
                a = int(p[0].split(':')[0].strip(), 16)
            except ValueError:
                continue
            # raw-bytes form: p[0] = "addr: bytes ", p[1] = bytes padding? the mnemonic is the first token that is not hex-pair bytes
            toks = [t for t in p[1:] if t != '']
            if toks and re.fullmatch(r'(?:[0-9a-f]{2} ?)+\s*', toks[0] + ' ') and len(toks) > 1 and not toks[0].isalpha():
                toks = toks[1:]
            if not toks:
                continue
            mn = toks[0].strip(); ops = toks[1].strip() if len(toks) > 1 else ''
            while mn in cf.PREFIXES and ops:
                sp = ops.split(None, 1); mn = sp[0]; ops = sp[1] if len(sp) > 1 else ''
            t = 0
            if mn.startswith('j') or mn == 'call':
                m = re.match(r'(?:0x)?([0-9a-f]+)\s*(?:<.*)?$', ops.strip())
                if m and not ops.startswith('qword') and '[' not in ops:
                    t = int(m.group(1), 16)
            elif mn in cf.SHIFTS:
                last = ops.rsplit(',', 1)[-1].strip()
                if re.fullmatch(r'(0x)?[0-9a-f]+', last) and not last.startswith('cl'):
                    t = 1
            i = len(self.addr)
            self.addr.append(a); self.mn.append(self.mid(mn)); self.tgt.append(t); self.off.append(offsets[k] if offsets else 0)
            if mn in ('vpsllvq', 'vpsrlvq'): self.sites['shiftv'].append(i)
            elif mn in ('vptest', 'vtestps', 'vtestpd'): self.sites['ptest'].append(i)
        self.n = len(self.addr)

    def load_file(self, path):
        self.f = open(path, 'rb'); self.buf = mmap.mmap(self.f.fileno(), 0, access=mmap.ACCESS_READ)
        offs = []; lines = []
        # stream: decode line by line without keeping the text
        def gen():
            pos = 0
            for raw in iter(self.buf.readline, b''):
                offs.append(pos); pos += len(raw); yield raw.decode('ascii', 'replace').rstrip('\n')
        self.load(gen(), offs)
        self.offs_list = None

    def text(self, i):
        """(mnemonic, [operands]) of instruction i, operands split on commas outside brackets."""
        if self.buf is not None:
            end = self.buf.find(b'\n', self.off[i]); line = self.buf[self.off[i]:end].decode('ascii', 'replace')
            raw = line
        else:
            raw = self.lines[i]
        p = [t for t in raw.split('\t') if t != ''][1:]
        if p and re.fullmatch(r'(?:[0-9a-f]{2} ?)+\s*', p[0] + ' ') and len(p) > 1 and not p[0].isalpha(): p = p[1:]
        mn = p[0].strip(); ops = p[1].strip() if len(p) > 1 else ''
        while mn in cf.PREFIXES and ops:
            sp = ops.split(None, 1); mn = sp[0]; ops = sp[1] if len(sp) > 1 else ''
        comment = ''
        if '#' in ops: ops, comment = ops.split('#', 1); ops = ops.strip(); comment = comment.strip()
        out = []; depth = 0; cur = ''
        for ch in ops:
            if ch == '[': depth += 1
            if ch == ']': depth -= 1
            if ch == ',' and depth == 0: out.append(cur.strip()); cur = ''
            else: cur += ch
        if cur.strip(): out.append(cur.strip())
        return mn, out, comment

    def build_edges(self):
        t = [(int(self.tgt[i]), i) for i in range(self.n) if self.tgt[i] > 1 and (self.mlist[self.mn[i]].startswith('j'))]
        self.isbt = set(); self.preds = collections.defaultdict(list)
        for ta, src in t:
            j = bisect.bisect_left(self.addr, ta)
            if j < self.n and self.addr[j] == ta:
                self.isbt.add(j); self.preds[j].append(src)

    def falls(self, i):
        """can control fall from instruction i to i+1?"""
        m = self.mlist[self.mn[i]]
        return m not in ('ret', 'retq', 'jmp', 'int3', 'ud2', 'hlt', 'iret', 'iretq', 'ud1')

# ------------------------------------------------------------------ image constants
class Image:
    def __init__(self, path=None, data=None):
        self.secs = []
        if path:
            self.data = open(path, 'rb').read()
            d = self.data; pe = struct.unpack_from('<I', d, 0x3c)[0]
            nsec = struct.unpack_from('<H', d, pe + 6)[0]; opt = struct.unpack_from('<H', d, pe + 20)[0]; so = pe + 24 + opt
            self.base = struct.unpack_from('<Q', d, pe + 24 + 24)[0]
            for k in range(nsec):
                name, vs, va, rs, ro = struct.unpack_from('<8sIIII', d, so + 40 * k); self.secs.append((va, max(vs, rs), ro, rs, name.rstrip(b'\0').decode()))
        self.fake = data or {}
    def read(self, addr, n):
        if addr in self.fake: return self.fake[addr][:n]
        rva = addr - getattr(self, 'base', BASE)
        for va, vs, ro, rs, name in self.secs:
            if va <= rva < va + vs:
                o = rva - va
                if o + n <= rs: return self.data[ro + o: ro + o + n]
                return None
        return None

# ------------------------------------------------------------------ gap 1: backward value analysis
REG = re.compile(r'^[xyz]mm(\d+)$')
GPR64 = {'rax': 0, 'rcx': 1, 'rdx': 2, 'rbx': 3, 'rsp': 4, 'rbp': 5, 'rsi': 6, 'rdi': 7}
GPR32 = {'eax': 0, 'ecx': 1, 'edx': 2, 'ebx': 3, 'esp': 4, 'ebp': 5, 'esi': 6, 'edi': 7}
GPR16 = {'ax': 0, 'cx': 1, 'dx': 2, 'bx': 3, 'sp': 4, 'bp': 5, 'si': 6, 'di': 7}
GPR8 = {'al': 0, 'cl': 1, 'dl': 2, 'bl': 3, 'sil': 6, 'dil': 7}
for k in range(8, 16):
    GPR64[f'r{k}'] = k; GPR32[f'r{k}d'] = k; GPR16[f'r{k}w'] = k; GPR8[f'r{k}b'] = k
VOLATILE_X = set(range(0, 6))

def vreg(op):
    m = REG.match(op); return int(m.group(1)) if m else None
def greg(op):
    """('gpr', index, bits) or None"""
    for tab, bits in ((GPR64, 64), (GPR32, 32), (GPR16, 16), (GPR8, 8)):
        if op in tab: return (tab[op], bits)
    return None
def num(s):
    try: return int(s, 0)
    except ValueError: return None
def bl(b): return (1 << b.bit_length()) - 1

def U(v):
    if v is None or v == 'bot': return None
    if v[0] == 'ub' or v[0] == 'ind': return v[1]
    if v[0] == 'c': return max(v[1])
    return None

class Tracer:
    def __init__(self, dis, img):
        self.d = dis; self.img = img; self.memo = {}; self.cul = []
    # --- abstract values: ('c', lanes) exact qword lanes; ('ub', B); ('mask',); ('ind', B0) loop counter starting <= B0; None unknown; 'bot' (no information yet)
    def join(self, a, b):
        if a == 'bot': return b
        if b == 'bot': return a
        if a is None or b is None: return None
        if a == b: return a
        ka, kb = a[0], b[0]
        if ka == 'ind' or kb == 'ind':
            ua, ub_ = U(a), U(b)
            return ('ind', max(ua, ub_)) if ua is not None and ub_ is not None and ka in ('ind', 'ub', 'c') and kb in ('ind', 'ub', 'c') else None
        if ka == 'mask' or kb == 'mask':
            other = b if ka == 'mask' else a
            return ('ub', M64) if (other[0] != 'mask' and U(other) is not None and U(other) < (1 << 32)) else None
        ua, ub_ = U(a), U(b)
        return ('ub', max(ua, ub_))
    def safe(self, v):
        if v == 'bot': return True
        if v is None: return False
        if v[0] == 'mask': return True
        if v[0] == 'c': return all((q >> 32) == 0 or (q & 0xFFFFFFFF) >= 64 for q in v[1])
        if v[0] == 'ub': return v[1] < (1 << 32)
        return False
    def induction(self, v): return v is not None and v != 'bot' and v[0] == 'ind' and v[1] < (1 << 32)
    def const_lanes(self, comment, opnd, nbytes):
        m = re.search(r'0x([0-9a-f]+)', comment)
        if not m: return None
        raw = self.img.read(int(m.group(1), 16), nbytes)
        if raw is None or len(raw) < nbytes: return None
        return raw
    def mem_size(self, opnd):
        for k, n in (('ymmword', 32), ('xmmword', 16), ('qword', 8), ('dword', 4), ('word ptr', 2), ('byte', 1)):
            if k in opnd: return n
        return None
    def const_val(self, opnd, comment, lanes_bytes, mode):
        """abstract value of a rip-relative constant: mode 'q' (qword lanes) or 'dbroadcast'/'qbroadcast'."""
        if 'rip' not in opnd: return None
        raw = self.const_lanes(comment, opnd, lanes_bytes)
        if raw is None: return None
        if mode == 'q': lanes = [struct.unpack_from('<Q', raw, 8 * k)[0] for k in range(lanes_bytes // 8)]
        elif mode == 'q1': lanes = [struct.unpack_from('<Q', raw, 0)[0]]
        elif mode == 'd1': d = struct.unpack_from('<I', raw, 0)[0]; lanes = [d | (d << 32)]
        elif mode == 'dz': lanes = [struct.unpack_from('<I', raw, 0)[0]]
        else: return None
        return ('c', tuple(lanes))
    # --- value of vector register r just before instruction i
    def vval(self, r, i, depth=0):
        key = ('v', r, i)
        if key in self.memo:
            v = self.memo[key]; return 'bot' if v == 'inprog' else v
        self.memo[key] = 'inprog'; steps = [0]
        res = self.scan('v', r, i, steps, depth)
        self.memo[key] = res; return res
    def gval(self, r, i, depth=0):
        key = ('g', r, i)
        if key in self.memo:
            v = self.memo[key]; return 'bot' if v == 'inprog' else v
        self.memo[key] = 'inprog'; steps = [0]
        res = self.scan('g', r, i, steps, depth)
        self.memo[key] = res; return res
    def preds_of(self, i):
        d = self.d; out = []
        if i - 1 >= 0 and d.falls(i - 1): out.append(i - 1)
        if i in d.isbt: out.extend(d.preds[i])
        return out
    def scan(self, kind, r, i, steps, depth):
        """walk back from instruction i (exclusive) over all predecessors; join the values found"""
        acc = 'bot'; seen = set(); ps = self.preds_of(i)
        if not ps: return None                          # function entry: an argument, unknown
        stack = list(ps)
        while stack:
            p = stack.pop()
            if p in seen: continue
            seen.add(p); steps[0] += 1
            if steps[0] > STEPS: return None
            res = self.effect(kind, r, p, depth)
            if isinstance(res, str) and res == 'cont':
                q = self.preds_of(p)
                if not q: acc = self.join(acc, None); self.cul.append(f'entry {self.d.addr[p] - BASE:#x}')
                else: stack.extend(q)
            else:
                if res is None: self.cul.append(f'{self.d.addr[p] - BASE:#x}: ' + ' '.join(self.d.text(p)[:2][0:1]) + ' ' + ', '.join(self.d.text(p)[1]))
                acc = self.join(acc, res)
        return acc
    def effect(self, kind, r, p, depth):
        """what instruction p does to register r: 'cont' (nothing) or an abstract value."""
        d = self.d; mn, ops, cm = d.text(p)
        if mn == 'call':
            if kind == 'v' and r in VOLATILE_X: return None
            if kind == 'g' and r in (0, 1, 2, 8, 9, 10, 11): return None
            return 'cont'
        if mn in ('syscall',): return None
        if kind == 'v': return self.veffect(r, mn, ops, cm, p, depth)
        return self.geffect(r, mn, ops, cm, p, depth)
    def operand_v(self, op, p, depth):
        v = vreg(op)
        return self.vval(v, p, depth + 1) if v is not None else None
    def veffect(self, r, mn, ops, cm, p, depth):
        if not ops: return 'cont'
        d0 = vreg(ops[0])
        if d0 != r:
            # instructions that write xmm/ymm through a non-first operand are rare; vzeroupper/vzeroall clobber upper halves/all
            if mn == 'vzeroall': return ('ub', 0)
            return 'cont'
        src = ops[1:] ; zeros = ('vpxor', 'vxorps', 'vxorpd', 'pxor', 'xorps', 'xorpd')
        if mn in zeros and len(src) == 2 and src[0] == src[1]: return ('ub', 0)
        ub = lambda x: x if x is not None else None
        def V(k):   # value of source operand k
            o = src[k]
            if vreg(o) is not None: return self.vval(vreg(o), p, depth + 1)
            if 'rip' in o: return self.const_val(o, cm, self.mem_size(o) or 32, 'q')
            if '[rsp' in o: return self.slot(o, p, depth)
            return None
        if mn in ('vmovdqa', 'vmovdqu', 'vmovaps', 'vmovups', 'vmovapd', 'vmovupd', 'vlddqu', 'movdqa', 'movdqu', 'movaps', 'movups'):
            s = src[0]
            if vreg(s) is not None: return V(0)
            if '[rsp' in s and 'rip' not in s: return self.slot(s, p, depth)
            n = self.mem_size(s) or (32 if ops[0].startswith('ymm') else 16)
            return self.const_val(s, cm, n, 'q')
        if mn in ('vmovd', 'movd'):
            s = src[0]
            if vreg(s) is not None: return None
            return ('ub', 0xFFFFFFFF)                    # zero-extended 32 bits (from a general register or memory)
        if mn in ('vmovq', 'movq'):
            s = src[0]
            if vreg(s) is not None: return V(0)
            g = greg(s)
            if g: return self.gval(g[0], p, depth + 1) if g[1] == 64 else ('ub', (1 << g[1]) - 1)
            return self.const_val(s, cm, 8, 'q1')
        if mn in ('vpbroadcastq', 'vmovddup', 'vbroadcastsd'):
            s = src[0]
            if vreg(s) is not None: return V(0)
            return self.const_val(s, cm, 8, 'q1')
        if mn in ('vpbroadcastd', 'vbroadcastss'):
            s = src[0]
            if vreg(s) is not None:
                v = V(0)
                return ('ub', 0) if U(v) == 0 else None
            return self.const_val(s, cm, 4, 'd1')
        if mn in ('vpmovzxdq', 'vpmovzxwq', 'vpmovzxbq') and vreg(src[0]) is not None: return ('ub', {'vpmovzxdq': 0xFFFFFFFF, 'vpmovzxwq': 0xFFFF, 'vpmovzxbq': 0xFF}[mn])
        if mn == 'vpsadbw': return ('ub', 8 * 255)
        if mn in ('vpand', 'vandps', 'vandpd', 'pand', 'andps', 'andpd'):
            a, b = (V(0), V(1)) if len(src) == 2 else (None, None)
            if len(src) == 2 and vreg(src[1]) is None: b = self.const_val(src[1], cm, self.mem_size(src[1]) or 32, 'q')
            return self.land(a, b)
        if mn in ('vpandn', 'vandnps', 'vandnpd'):
            b = V(1) if vreg(src[1]) is not None else self.const_val(src[1], cm, self.mem_size(src[1]) or 32, 'q')
            return ('ub', b[1]) if b not in (None, 'bot') and b[0] == 'ub' else (b if b == 'bot' else None)
        if mn in ('vpor', 'vorps', 'vorpd', 'vpxor', 'vxorps', 'vxorpd'):
            a = V(0); b = V(1) if vreg(src[1]) is not None else self.const_val(src[1], cm, self.mem_size(src[1]) or 32, 'q')
            if U(a) is not None and U(b) is not None: return ('ub', bl(max(U(a), U(b))))
            return None
        if mn in ('vpminuq',):
            a = V(0); b = V(1) if vreg(src[1]) is not None else self.const_val(src[1], cm, 32, 'q')
            return self.land(a, b)
        if mn in ('vpsrlq', 'vpsllq') and len(src) == 2 and num(src[1]) is not None:
            k = num(src[1]); a = V(0); ua = U(a) if a not in (None,) else None
            if mn == 'vpsrlq': return ('ub', (ua if ua is not None else M64) >> k) if k < 64 else ('ub', 0)
            if ua is not None and (ua << k) <= M64 and k < 64: return ('ub', ua << k)
            return None
        if mn in ('vpsrld', 'vpslld') and len(src) == 2 and num(src[1]) is not None:
            a = V(0)
            if U(a) is not None and U(a) < (1 << 32): return ('ub', 0xFFFFFFFF)       # a lane < 2^32 stays < 2^32 (dword op on [low, 0])
            return None
        if mn in ('vpaddq',) and len(src) == 2:
            a = V(0); b = V(1)
            if a == 'bot' and U(b) is not None and U(b) < (1 << 16): return ('ind', U(b))          # r = r + small step around a loop: a counter
            if b == 'bot' and U(a) is not None and U(a) < (1 << 16): return ('ind', U(a))
            if a is not None and a != 'bot' and a[0] == 'ind' and U(b) is not None and U(b) < (1 << 16): return ('ind', U(a))
            if b is not None and b != 'bot' and b[0] == 'ind' and U(a) is not None and U(a) < (1 << 16): return ('ind', U(b))
            if U(a) is not None and U(b) is not None and U(a) + U(b) <= M64: return ('ub', U(a) + U(b))
            return None
        if mn == 'vpshufb' and len(src) == 2:
            ctl = V(1) if vreg(src[1]) is not None else self.const_val(src[1], cm, self.mem_size(src[1]) or 16, 'q')
            if ctl is not None and ctl != 'bot' and ctl[0] == 'c':
                raw = b''.join(struct.pack('<Q', q) for q in ctl[1]); top = -1
                for k, byte in enumerate(raw):
                    if not (byte & 0x80) and (k % 8) > top: top = k % 8
                return ('ub', (1 << (8 * (top + 1))) - 1) if top >= 0 else ('ub', 0)
            return None
        if mn == 'vpblendd' and len(src) == 3 and num(src[2]) is not None:
            imm = num(src[2]); a = V(0); b = V(1) if vreg(src[1]) is not None else None
            nd = 8 if ops[0].startswith('ymm') else 4
            for k in range(1, nd, 2):                     # the odd dwords are the high halves of the qword lanes
                srcv = b if (imm >> k) & 1 else a
                if not (srcv is not None and srcv != 'bot' and U(srcv) is not None and U(srcv) < (1 << 32)): return None
            return ('ub', 0xFFFFFFFF)
        if mn.startswith('vpmovsx') or mn.startswith('vpmovzx'):
            sz = {'bw': (1, 2), 'bd': (1, 4), 'bq': (1, 8), 'wd': (2, 4), 'wq': (2, 8), 'dq': (4, 8)}.get(mn[-2:])
            s0 = src[0]
            if sz and vreg(s0) is None and 'rip' in s0:
                dest_bytes = 32 if ops[0].startswith('ymm') else 16; cnt = dest_bytes // sz[1]; nb = cnt * sz[0]
                raw = self.const_lanes(cm, s0, nb)
                if raw is None: return None
                out = b''
                for k in range(cnt):
                    v = int.from_bytes(raw[k * sz[0]:(k + 1) * sz[0]], 'little')
                    if mn.startswith('vpmovsx') and v >> (8 * sz[0] - 1): v |= ((1 << (8 * sz[1])) - 1) ^ ((1 << (8 * sz[0])) - 1)
                    out += v.to_bytes(sz[1], 'little')
                return ('c', tuple(struct.unpack_from('<Q', out, 8 * k)[0] for k in range(len(out) // 8)))
            return None
        if mn in ('vpcmpeqq', 'vpcmpeqd', 'vpcmpeqb', 'vpcmpeqw', 'vpcmpgtq', 'vpcmpgtd'):
            return ('mask',) if mn.endswith('q') else None        # a dword compare gives per-dword masks: a lane may be 0x00000000ffffffff or 0xffffffff00000000
        if mn in ('vpinsrq', 'vpinsrd', 'vpinsrw', 'vpinsrb'): return None
        return None                                         # anything else: unknown
    def slot(self, opnd, p, depth):
        """value stored in the stack slot `opnd` by the nearest earlier store to the same operand text, with no rsp change in between"""
        d = self.d; i = p; steps = 0
        while i > 0 and steps < 400:
            i -= 1; steps += 1
            if i in d.isbt and i != p - 0 and steps > 0 and False: return None
            mn, ops, cm = d.text(i)
            if mn in ('call',) or (ops and ops[0] in ('rsp',) ) or mn in ('push', 'pop', 'leave', 'enter'): return None
            if mn.startswith('v') or mn.startswith('mov'):
                if len(ops) >= 2 and ops[0] == opnd and vreg(ops[1]) is not None: return self.vval(vreg(ops[1]), i, depth + 1)
                if ops and ops[0] == opnd: return None
            if not d.falls(i - 1) if i > 0 else True: return None
        return None
    def land(self, a, b):
        if a == 'bot' or b == 'bot': return 'bot' if (a == 'bot' and b == 'bot') else (b if a == 'bot' else a)
        if a is not None and b is not None and a[0] == 'mask' and b[0] == 'mask': return a
        ua = U(a); ub_ = U(b)
        if ua is not None and ub_ is not None: return ('ub', min(ua, ub_))
        if ua is not None: return ('ub', ua)
        if ub_ is not None: return ('ub', ub_)
        return None
    def geffect(self, r, mn, ops, cm, p, depth):
        """general-register value for vmovq xmm, r64 counts."""
        if not ops: return 'cont'
        g = greg(ops[0])
        writes = g is not None and g[0] == r
        if mn in ('cdq', 'cdqe', 'cqo', 'cwd', 'cwde') or mn in ('mul', 'imul', 'div', 'idiv') and not (len(ops) > 1 and g):
            if r in (0, 2): return None
            return 'cont'
        if mn in ('cmp', 'test', 'push', 'jmp') or mn.startswith('j') or mn.startswith('prefetch') or mn.startswith('v') and not writes:
            return 'cont' if not (mn in ('push',)) else 'cont'
        if not writes:
            if mn in ('xchg', 'xadd', 'cmpxchg') and len(ops) > 1 and greg(ops[1]) and greg(ops[1])[0] == r: return None
            if mn == 'pop': return 'cont'
            if mn in ('movs', 'movsq', 'stos', 'stosq', 'rep'): return 'cont'
            return 'cont'
        bits = g[1]
        if bits == 32 and mn not in ('cmp', 'test'): return ('ub', 0xFFFFFFFF) if mn != 'xchg' else None     # 32-bit writes zero-extend
        if bits < 32: return None                      # partial write keeps old high bits
        # 64-bit destination
        if mn == 'mov' and len(ops) == 2:
            n_ = num(ops[1])
            if n_ is not None: return ('ub', n_ & M64)
            g2 = greg(ops[1])
            if g2: return self.gval(g2[0], p, depth + 1) if g2[1] == 64 else ('ub', (1 << g2[1]) - 1)
            return None
        if mn == 'xor' and len(ops) == 2 and ops[0] == ops[1]: return ('ub', 0)
        if mn in ('movzx',): return ('ub', 0xFFFF)
        if mn == 'and' and len(ops) == 2:
            n_ = num(ops[1]); old = self.gval(r, p, depth + 1)
            if n_ is not None:
                if n_ < 0: n_ &= M64
                return ('ub', min(n_, old[1])) if old not in (None, 'bot') and old[0] == 'ub' else ('ub', n_)
            return None
        if mn in ('shr',) and len(ops) == 2 and num(ops[1]) is not None:
            k = num(ops[1]); return ('ub', M64 >> k) if k < 64 else ('ub', 0)
        if mn == 'lea' or mn in ('add', 'sub', 'neg', 'not', 'imul', 'shl', 'sar', 'or', 'xor', 'movsxd', 'movsx', 'popcnt', 'lzcnt', 'tzcnt', 'bsr', 'bsf'):
            if mn in ('popcnt', 'lzcnt', 'tzcnt', 'bsr', 'bsf'): return ('ub', 64)
            return None
        return None

# ------------------------------------------------------------------ gap 1 driver
def analyse_shifts(dis, img, outdir=None):
    tr = Tracer(dis, img); rows = []; res = collections.Counter(); why = collections.Counter()
    for i in dis.sites['shiftv']:
        mn, ops, cm = dis.text(i)
        if len(ops) < 3: rows.append((i, mn, 'suspect', 'unparsed')); res['suspect'] += 1; continue
        c = ops[2]; steps0 = len(tr.memo)
        if vreg(c) is not None:
            tr.memo = {} if len(tr.memo) > 2000000 else tr.memo
            v = tr.vval(vreg(c), i)
            how = 'register'
        else:
            n = tr.mem_size(c) or 16
            v = tr.const_val(c, cm, n, 'q'); how = 'memory operand'
            if v is None: how = 'memory operand (not a rip-relative constant)'
        if tr.cul and not tr.safe(v): how += ' <- ' + ' | '.join(tr.cul[-3:])
        tr.cul = []
        if tr.safe(v): cls = 'safe'
        elif tr.induction(v): cls = 'induction'
        else: cls = 'suspect'
        res[cls] += 1
        rows.append((i, mn, cls, f'{how}: {v}'))
        if cls != 'safe': why[(mn, cls, how)] += 1
    return rows, res, why

# ------------------------------------------------------------------ gap 2 driver
PF_LIVE = cf.P
def flag_kinds(dis):
    return [cf.classify(m) for m in dis.mlist]

def walk_pf(dis, kind, start, live0=PF_LIVE, limit=cf.STEPS):
    n = dis.n; seen = set(); stack = [(start, live0, 0)]; unresolved = None
    while stack:
        i, live, steps = stack.pop()
        while live:
            if i >= n: unresolved = unresolved or 'ran off the text'; break
            if (i, live) in seen: break
            seen.add((i, live)); steps += 1
            if steps > limit: unresolved = unresolved or f'more than {limit} instructions'; break
            k = kind[dis.mn[i]]; t = k[0]
            if t == 'end': break
            if t == 'reads':
                if k[1] & live: return ('reader', dis.mlist[dis.mn[i]], dis.addr[i] - BASE, steps)
            elif t == 'jcc':
                if k[1] & live: return ('reader', dis.mlist[dis.mn[i]], dis.addr[i] - BASE, steps)
                tgt = _idx(dis, dis.tgt[i]) if dis.tgt[i] > 1 else None
                if tgt is None: unresolved = unresolved or f'branch to {dis.tgt[i]:#x} out of the decoded instructions'
                else: stack.append((tgt, live, steps))
            elif t == 'jmp':
                if dis.tgt[i] > 1:
                    tgt = _idx(dis, dis.tgt[i])
                    if tgt is None: unresolved = unresolved or f'jmp to {dis.tgt[i]:#x} out of the decoded instructions'; break
                    i = tgt; continue
                break
            elif t == 'writes':
                w = k[1]
                if dis.mlist[dis.mn[i]] in cf.SHIFTS and dis.tgt[i] != 1: w = 0
                live &= ~w
            i += 1
    return ('unresolved', unresolved) if unresolved else ('safe',)

def _idx(dis, a):
    j = bisect.bisect_left(dis.addr, a)
    return j if j < dis.n and dis.addr[j] == a else None

def analyse_ptest(dis):
    kind = flag_kinds(dis); res = collections.Counter(); rows = []; by = collections.Counter(); bym = collections.Counter()
    for i in dis.sites['ptest']:
        r = walk_pf(dis, kind, i + 1, limit=4000); res[r[0]] += 1; bym[dis.mlist[dis.mn[i]]] += 1
        rows.append((i, dis.mlist[dis.mn[i]], r))
        if r[0] == 'reader': by[r[1]] += 1
    return rows, res, by, bym

# ------------------------------------------------------------------ report
def run(dis, img, outdir):
    dis.build_edges()
    srows, sres, swhy = analyse_shifts(dis, img)
    prows, pres, pby, pbym = analyse_ptest(dis)
    lines = [f'instructions {dis.n}',
             f'gap 1  vpsllvq/vpsrlvq sites {len(dis.sites["shiftv"])}: safe {sres["safe"]}  induction {sres["induction"]}  suspect {sres["suspect"]}',
             'suspect by (mnemonic, count origin): ' + '; '.join(f'{m} {k} {h[:40]} x{c}' for (m, k, h), c in swhy.most_common()),
             f'gap 2  vptest/vtestps/vtestpd sites {len(dis.sites["ptest"])} ({", ".join(f"{m} {c}" for m, c in pbym.most_common())}): safe {pres["safe"]}  reader {pres["reader"]}  unresolved {pres["unresolved"]}',
             'PF/AF readers by mnemonic: ' + ', '.join(f'{m} {c}' for m, c in pby.most_common())]
    if outdir:
        os.makedirs(outdir, exist_ok=True)
        with open(os.path.join(outdir, 'isa-gaps-vpsllvq-sites.tsv'), 'w') as f:
            f.write('rva\tmnemonic\tverdict\tcount_origin\n')
            for i, mn, cls, how in srows: f.write(f'{dis.addr[i] - BASE:#x}\t{mn}\t{cls}\t{how}\n')
        with open(os.path.join(outdir, 'isa-gaps-vptest-sites.tsv'), 'w') as f:
            f.write('rva\tmnemonic\tverdict\treader\treader_rva\tinstructions\n')
            for i, mn, r in prows:
                f.write(f'{dis.addr[i] - BASE:#x}\t{mn}\t{r[0]}\t' + (f'{r[1]}\t{r[2]:#x}\t{r[3]}' if r[0] == 'reader' else (r[1] if r[0] == 'unresolved' else '') + '\t\t') + '\n')
        open(os.path.join(outdir, 'isa-gaps-summary.txt'), 'w').write('\n'.join(lines) + '\n')
    return srows, sres, prows, pres, lines

# ------------------------------------------------------------------ selftest
def mk(listing):
    """listing: list of (mnemonic, operands[, comment]) at consecutive addresses; objdump no-raw-bytes form. Returns Dis."""
    d = Dis(); d.lines = []; txt = []
    a = BASE + 0x1000
    for item in listing:
        mn, ops = item[0], item[1]; cm = (' # ' + item[2]) if len(item) > 2 else ''
        txt.append(f'{a:x}:     \t{mn}\t{ops}{cm}'); a += 4
    d.lines = txt; d.load(txt); d.build_edges(); return d

def selftest():
    bad = 0
    img = Image(data={0x146000000: struct.pack('<QQ', 3, 63), 0x146000010: struct.pack('<QQ', 5, 0x100000000 + 5), 0x146000020: struct.pack('<QQ', 1 << 40, 2), 0x146000030: bytes([0,1,2,3,0x80,0x80,0x80,0x80, 8,9,10,11,0x80,0x80,0x80,0x80]), 0x146000040: bytes([0,1,2,3,0x80,5,0x80,0x80, 8,9,10,11,0x80,0x80,0x80,0x80]), 0x146000050: bytes([1,2,3,4]), 0x146000060: bytes([1,2,0xff,4])})
    def shift_verdict(*pre, count='xmm3'):
        d = mk(list(pre) + [('vpsllvq', f'xmm0, xmm1, {count}'), ('ret', '')]); tr = Tracer(d, img)
        i = d.sites['shiftv'][0]; mn, ops, cm = d.text(i)
        return 'safe' if tr.safe(tr.vval(vreg(ops[2]), i)) else 'suspect'
    A0 = BASE + 0x1000
    cases = [
        ('constant count below 64 loaded from the image', 'safe', [('vmovdqa', 'xmm3, xmmword ptr [rip + 0x100]', '0x146000000')]),
        ('constant count with a lane >= 2^32 (the gap)', 'suspect', [('vmovdqa', 'xmm3, xmmword ptr [rip + 0x100]', '0x146000010')]),
        ('constant with a lane 2^40', 'suspect', [('vmovdqa', 'xmm3, xmmword ptr [rip + 0x100]', '0x146000020')]),
        ('vmovd from a register zero-extends', 'safe', [('vmovd', 'xmm3, eax')]),
        ('vmovq from an unknown 64-bit register', 'suspect', [('vmovq', 'xmm3, rax')]),
        ('vmovq from a register that was a 32-bit write', 'safe', [('mov', 'eax, dword ptr [rdi]'), ('vmovq', 'xmm3, rax')]),
        ('vpand with a constant <= 63 clears an unknown value', 'safe', [('vpand', 'xmm3, xmm2, xmmword ptr [rip + 0x100]', '0x146000000')]),
        ('vpand with a constant >= 2^32 does not', 'suspect', [('vpand', 'xmm3, xmm2, xmmword ptr [rip + 0x100]', '0x146000010')]),
        ('vpminuq with a bound', 'safe', [('vmovdqa', 'xmm4, xmmword ptr [rip + 0x100]', '0x146000000'), ('vpminuq', 'xmm3, xmm2, xmm4')]),
        ('a subtraction', 'suspect', [('vpsubq', 'xmm3, xmm1, xmm2')]),
        ('a negation (0 - x)', 'suspect', [('vpxor', 'xmm4, xmm4, xmm4'), ('vpsubq', 'xmm3, xmm4, xmm2')]),
        ('an unknown memory load', 'suspect', [('vmovdqu', 'xmm3, xmmword ptr [rdi]')]),
        ('a compare mask', 'safe', [('vpcmpeqq', 'xmm3, xmm1, xmm2')]),
        ('zeroed', 'safe', [('vpxor', 'xmm3, xmm3, xmm3')]),
        ('vpsrlq by 40 of an unknown value', 'safe', [('vpsrlq', 'xmm3, xmm2, 40')]),
        ('vpsrlq by 3 of an unknown value', 'suspect', [('vpsrlq', 'xmm3, xmm2, 3')]),
        ('vpmovzxdq zero-extends', 'safe', [('vpmovzxdq', 'xmm3, xmm2')]),
        ('vpmovsxdq sign-extends', 'suspect', [('vpmovsxdq', 'xmm3, xmm2')]),
        ('a register written before a call that clobbers it', 'suspect', [('vpxor', 'xmm3, xmm3, xmm3'), ('call', f'{A0 + 0x400:#x}')]),
        ('xmm8 survives a call', 'safe', [('vpxor', 'xmm8, xmm8, xmm8'), ('call', f'{A0 + 0x400:#x}')]),
        ('function entry: an argument', 'suspect', []),
        ('pshufb whose control zeroes the high four bytes of each lane', 'safe', [('vmovdqa', 'xmm4, xmmword ptr [rip + 0x100]', '0x146000030'), ('vpshufb', 'xmm3, xmm2, xmm4')]),
        ('pshufb whose control keeps byte 5', 'suspect', [('vmovdqa', 'xmm4, xmmword ptr [rip + 0x100]', '0x146000040'), ('vpshufb', 'xmm3, xmm2, xmm4')]),
        ('pshufb with an unknown control', 'suspect', [('vpshufb', 'xmm3, xmm2, xmm4')]),
        ('blendd taking the odd dwords from a zero register', 'safe', [('vpxor', 'xmm5, xmm5, xmm5'), ('vpblendd', 'xmm3, xmm2, xmm5, 0xa')]),
        ('blendd taking the odd dwords from an unknown register', 'suspect', [('vpblendd', 'xmm3, xmm2, xmm5, 0x5')]),
        ('vpmovsxbq of a constant of small bytes', 'safe', [('vpmovsxbq', 'xmm3, word ptr [rip + 0x100]', '0x146000050')]),
        ('vpmovzxbq of a constant', 'safe', [('vpmovzxbq', 'xmm3, word ptr [rip + 0x100]', '0x146000060')]),
    ]
    for name, want, pre in cases:
        got = shift_verdict(*pre, count='xmm8' if 'xmm8' in name else 'xmm3')
        if got != want: bad += 1; print(f'FAIL shift {name}: want {want}, got {got}')
    # a join: both arms must be safe
    d = mk([('je', f'{A0 + 16:#x}'), ('vpxor', 'xmm3, xmm3, xmm3'), ('jmp', f'{A0 + 20:#x}'), ('nop', ''), ('vpsubq', 'xmm3, xmm1, xmm2'), ('vpsllvq', 'xmm0, xmm1, xmm3')])
    tr = Tracer(d, img); i = d.sites['shiftv'][0]
    if tr.safe(tr.vval(3, i)): bad += 1; print('FAIL shift: join with an unsafe arm')
    d = mk([('je', f'{A0 + 16:#x}'), ('vpxor', 'xmm3, xmm3, xmm3'), ('jmp', f'{A0 + 20:#x}'), ('nop', ''), ('vpxor', 'xmm3, xmm3, xmm3'), ('vpsllvq', 'xmm0, xmm1, xmm3')])
    tr = Tracer(d, img); i = d.sites['shiftv'][0]
    if not tr.safe(tr.vval(3, i)): bad += 1; print('FAIL shift: join with two safe arms')
    d = mk([('vmovdqa', 'xmm7, xmmword ptr [rip + 0x100]', '0x146000000'), ('vpxor', 'xmm3, xmm3, xmm3'), ('vpsllvq', 'xmm0, xmm1, xmm3'), ('vpaddq', 'xmm3, xmm3, xmm7'), ('jmp', f'{A0 + 8:#x}')])
    rows, res, why = analyse_shifts(d, img)
    if res['induction'] != 1: bad += 1; print('FAIL shift: loop counter', dict(res))
    # memory operand count straight in the instruction
    d = mk([('vpsllvq', 'xmm0, xmm1, xmmword ptr [rip + 0x100]', '0x146000010')]); tr = Tracer(d, img)
    rows, res, why = analyse_shifts(d, img)
    if res['suspect'] != 1: bad += 1; print('FAIL shift: memory operand with a high lane')
    d = mk([('vpsllvq', 'xmm0, xmm1, xmmword ptr [rip + 0x100]', '0x146000000')]); rows, res, why = analyse_shifts(d, img)
    if res['safe'] != 1: bad += 1; print('FAIL shift: memory operand constant < 64')
    d = mk([('vpsllvq', 'xmm0, xmm1, xmmword ptr [rdi]')]); rows, res, why = analyse_shifts(d, img)
    if res['suspect'] != 1: bad += 1; print('FAIL shift: unknown memory operand')
    # ptest: PF/AF readers
    pc = [
        ('ZF branch only', 'safe', [('jne', f'{A0 + 20:#x}'), ('ret', ''), ('ret', ''), ('ret', ''), ('ret', '')]),
        ('jp right after', 'reader', [('jp', f'{A0 + 20:#x}'), ('ret', '')]),
        ('jnp right after', 'reader', [('jnp', f'{A0 + 20:#x}'), ('ret', '')]),
        ('setp', 'reader', [('setp', 'al'), ('ret', '')]),
        ('cmovnp', 'reader', [('cmovnp', 'eax, ebx'), ('ret', '')]),
        ('lahf', 'reader', [('lahf', ''), ('ret', '')]),
        ('pushfq', 'reader', [('pushfq', ''), ('ret', '')]),
        ('ja (CF, ZF only) is not a PF reader', 'safe', [('ja', f'{A0 + 20:#x}'), ('ret', ''), ('ret', ''), ('ret', ''), ('ret', '')]),
        ('jl (SF, OF) is not a PF reader', 'safe', [('jl', f'{A0 + 20:#x}'), ('ret', ''), ('ret', ''), ('ret', ''), ('ret', '')]),
        ('overwritten by test before jp', 'safe', [('test', 'eax, eax'), ('jp', f'{A0 + 20:#x}'), ('ret', '')]),
        ('inc keeps CF but writes PF', 'safe', [('inc', 'eax'), ('jp', f'{A0 + 20:#x}'), ('ret', '')]),
        ('adc reads CF only', 'safe', [('adc', 'eax, 0'), ('ret', '')]),
        ('call ends the path', 'safe', [('call', f'{A0 + 40:#x}'), ('jp', f'{A0 + 40:#x}')]),
        ('a reader on the taken side of a ZF branch', 'reader', [('je', f'{A0 + 16:#x}'), ('ret', ''), ('ret', ''), ('jp', f'{A0 + 40:#x}')]),
        ('shift by cl may not write flags', 'reader', [('shl', 'eax, cl'), ('jp', f'{A0 + 40:#x}')]),
    ]
    for mnm in ('vptest', 'vtestps', 'vtestpd'):
        for name, want, post in pc:
            d = mk([(mnm, 'xmm0, xmm1')] + post); kind = flag_kinds(d); got = walk_pf(d, kind, d.sites['ptest'][0] + 1)[0]
            if got == 'unresolved' and want == 'safe': got = 'safe?'
            if got != want: bad += 1; print(f'FAIL {mnm} {name}: want {want}, got {got}')
    tot = len(cases) + 3 + 3 + 1 + len(pc) * 3 + 2
    print(f'{tot - bad} of {tot} selftest cases pass'); return bad

if __name__ == '__main__':
    if len(sys.argv) > 1 and sys.argv[1] == '--selftest': sys.exit(1 if selftest() else 0)
    if len(sys.argv) < 4: print(__doc__); sys.exit(2)
    dis = Dis(); sys.stderr.write('decoding...\n'); dis.load_file(sys.argv[1])
    sys.stderr.write(f'{dis.n} instructions, {len(dis.sites["shiftv"])} vpsllvq/vpsrlvq, {len(dis.sites["ptest"])} vptest/vtest*\n')
    img = Image(sys.argv[2]); srows, sres, prows, pres, lines = run(dis, img, sys.argv[3]); print('\n'.join(lines))
