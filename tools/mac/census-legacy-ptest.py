#!/usr/bin/env python3
"""Audit PF/AF after legacy PTEST in llvm-objdump -d --x86-asm-syntax=intel --no-show-raw-insn output. Addresses are VAs. Caller-visible flags are dead at ABI call/return boundaries. Unknown instructions and indirect jumps are unresolved, never safe."""
import argparse, collections, re

def decode(lines):
    ins=[]
    for line in lines:
        m=re.match(r'^([0-9a-f]+):\s+(\S+)(?:\s+(.*?))?\s*$',line)
        if m: ins.append((int(m[1],16),m[2],m[3] or ''))
    return ins

def walk(ins, addr, i):
    todo=[i]; seen=set(); uncertain=None
    while todo:
        i=todo.pop()
        if i in seen: continue
        seen.add(i)
        if i>=len(ins): uncertain='end-of-input'; continue
        va,op,arg=ins[i]
        if op in ('jp','jpe','jnp','jpo','setp','setnp','setpe','setpo','cmovp','cmovnp','cmovpe','cmovpo','lahf','pushf','pushfq','pushfw'):
            return 'reader',f'{va:#x}:{op}'
        if op in ('ret','retq','call','callq','int3','ud2'): continue
        if op in ('add','sub','inc','dec','cmp','neg','sahf','popfq','ucomiss','comiss','ucomisd','comisd'): continue
        # Logical ops overwrite PF and make AF undefined. Incoming AF is not an architecturally defined dependency.
        if op in ('test','and','or','xor'): continue
        if op.startswith('j'):
            m=re.match(r'0x([0-9a-f]+)',arg); j=addr.get(int(m[1],16)) if m else None
            if j is None: uncertain=f'{va:#x}:unknown target'
            else: todo.append(j)
            if op=='jmp': continue
        elif not (op.startswith(('mov','vmov','cmov','set','nop')) or op in ('lea','push','pop','xchg','vzeroupper','lfence','pshufb','pmovmskb','btr')):
            uncertain=f'{va:#x}:{op}'; continue
        todo.append(i+1)
    return ('unresolved',uncertain) if uncertain else ('safe','')

def audit(ins):
    addr={va:i for i,(va,_,_) in enumerate(ins)}
    return [(va,*walk(ins,addr,i+1)) for i,(va,op,_) in enumerate(ins) if op=='ptest']

def selftest():
    for tail,want in [(['jp 0x20'],'reader'),(['lahf'],'reader'),(['pushfq'],'reader'),(['test eax,eax','jp 0x20'],'safe'),(['jmp rax'],'unresolved'),(['jne 0x18','ret','lahf'],'reader'),(['jne 0x18','ret','cmp eax,edx'],'safe')]:
        ins=decode([f'{16+2*i:x}: {op}' for i,op in enumerate(['ptest xmm0,xmm1']+tail)]) if len(tail)<3 else decode([f'{16+2*i:x}: {op}' for i,op in enumerate(['ptest xmm0,xmm1']+tail+['ret'])]);
        if len(tail)==3: ins[1]=(ins[1][0],'jne','0x16')
        assert audit(ins)[0][1]==want,(tail,audit(ins))
    print('7/7 (including deliberate PF and AF readers)')

if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('dis',nargs='?');p.add_argument('--selftest',action='store_true');args=p.parse_args()
    if args.selftest:selftest()
    else:
        with open(args.dis) as f: rows=audit(decode(f))
        print('va\tverdict\tevidence');
        for va,status,evidence in rows:print(f'{va:#x}\t{status}\t{evidence}')
