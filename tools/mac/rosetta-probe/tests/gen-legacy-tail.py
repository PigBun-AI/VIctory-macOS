#!/usr/bin/env python3
"""Generate a standalone Wine probe for remaining title SSE arithmetic and move forms.
Compare legacy against previously validated VEX semantics; separately require legacy upper 128 bit preservation.
Usage: python3 gen-legacy-tail.py > tail.c; x86_64-w64-mingw32-clang -O2 -mavx2 tail.c -o tail.exe
"""
forms=[]
def add(name,body,ref,vec=True):forms.append((name,body,ref,vec))
for m in 'addsd addss mulsd subsd subss divsd divss'.split():
    for typ,b in [('reg','%%xmm2'),('mem','(%[b])')]:add(m+'_'+typ,f'{m} {b},%%xmm1',f'v{m} {b},%%xmm1,%%xmm1')
for m in ['cvtsi2sd','cvtsi2ss']:
    for typ,b in [('reg','%%rax'),('mem','(%[b])')]:add(m+'_'+typ,f'{m}q {b},%%xmm1',f'v{m}q {b},%%xmm1,%%xmm1')
for m in ['cvttsd2si','cvttss2si']:
    for reg in ['%%rax','%%eax']:
        for typ,b in [('reg','%%xmm2'),('mem','(%[b])')]:add(m+'_'+reg[2:]+'_'+typ,f'{m} {b},{reg}',f'v{m} {b},{reg}',False)
for m in ['comiss','comisd']:
    for typ,b in [('reg','%%xmm2'),('mem','(%[b])')]:add(m+'_'+typ,f'{m} {b},%%xmm1;pushfq;pop %%rax;and $0x8d5,%%eax',f'v{m} {b},%%xmm1;pushfq;pop %%rax;and $0x8d5,%%eax',False)
for b in ['%%xmm2','(%[b])']:add('cvtdq2pd_'+('reg' if b[0]=='%' else 'mem'),f'cvtdq2pd {b},%%xmm1',f'vcvtdq2pd {b},%%xmm1')
for m,r in [('pinsrq','%%rax'),('pinsrd','%%eax')]:
    for i in range(2 if m=='pinsrq' else 4):
        for typ,b in [('mem','(%[b])'),('reg',r)]:add(f'{m}_{i}_{typ}',f'{m} ${i},{b},%%xmm1',f'v{m} ${i},{b},%%xmm1,%%xmm1')
add('pmovmskb','pmovmskb %%xmm2,%%eax','vpmovmskb %%xmm2,%%eax',False)
for m in ['movaps','movups','movdqa','movdqu','movsd','movss']:
    add(m+'_load',m+' (%[b]),%%xmm1','v'+m+' (%[b]),%%xmm1')
    add(m+'_store',m+' %%xmm2,(%[o])','v'+m+' %%xmm2,(%[o])',None)
    add(m+'_reg',m+' %%xmm2,%%xmm1','v'+m+(' %%xmm2,%%xmm1,%%xmm1' if m in ('movsd','movss') else ' %%xmm2,%%xmm1'))
for m,r in [('movd','%%eax'),('movq','%%rax')]:
    add(m+'_from_gpr',m+' '+r+',%%xmm1','v'+m+' '+r+',%%xmm1');add(m+'_to_gpr',m+' %%xmm2,'+r,'v'+m+' %%xmm2,'+r,False);add(m+'_load',m+' (%[b]),%%xmm1','v'+m+' (%[b]),%%xmm1')
print('''#include <immintrin.h>
#include <stdint.h>
#include <string.h>
#include <stdio.h>
#include <stdlib.h>
static uint64_t seed=0xad6f3b918;static uint64_t next(void){seed^=seed<<13;seed^=seed>>7;return seed^=seed<<17;}
typedef void(*fn)(void*,void*,void*);''')
for i,(name,body,ref,vec) in enumerate(forms):
    for j,ins in enumerate([body,ref]):
        post=(';vmovups %%ymm1,(%[o])' if vec is True else ';mov %%rax,(%[o])' if vec is False else '')
        print(f'static void f{i}_{j}(void*a,void*b,void*o){{__asm__ volatile("vmovups (%[a]),%%ymm1;vmovups (%[b]),%%ymm2;mov (%[b]),%%rax;{ins}{post}"::[a]"r"(a),[b]"r"(b),[o]"r"(o):"rax","ymm1","ymm2","cc","memory");}}')
print('static struct{const char*n;fn f,v;int vec;}ops[]={');
for i,(name,_,__,vec) in enumerate(forms):print(f'{{"{name}",f{i}_0,f{i}_1,{int(vec is True)}}},')
print('''};int main(void){_Alignas(32) uint64_t a[4],b[4],out[4],ref[4];uint64_t edges[]={0,1,~0ULL,0x8000000080000000ULL,0x7fc000017f800000ULL,0x7ff0000000000001ULL,0xfff0000000000000ULL,0x7f80000000000001ULL,0x7ff8000000000101ULL,0x43e0000000000000ULL,0xbfe0000000000000ULL,0x8000000000000001ULL};int neg=getenv("PROBE_NEG")!=NULL,bad=0;
for(unsigned i=0;i<sizeof(ops)/sizeof(*ops);i++){int mis=0;for(int k=0;k<131072;k++){if(!(k%8192))_mm_setcsr(0x1f80|(((k/8192)&3)<<13)|((((k/8192)>>2)&1)<<6)|((((k/8192)>>3)&1)<<15));for(int t=0;t<4;t++){a[t]=(k&1)?next():edges[next()%12];b[t]=(k&2)?next():edges[next()%12];ref[t]=out[t]=next();}ops[i].f(a,b,out);ops[i].v(a,b,ref);if(ops[i].vec)memcpy(ref+2,a+2,16);if(neg&&!k)ref[0]^=1;mis+=memcmp(ref,out,32)!=0;}printf("%s records=131072 mismatches=%d\\n",ops[i].n,mis);bad+=mis;}return bad!=0;}''')
