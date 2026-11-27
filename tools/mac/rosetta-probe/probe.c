// Rosetta 2 instruction probe (macOS, x86_64 slice run under Rosetta). Build/run: ../rosetta-probe.sh
// One instruction class per child process; with `fault` the destination page is made
// inaccessible and a SIGSEGV handler opens it and resumes (what Windows' VirtualProtect guards do).
// Prints: <case> <mode> exit=<n>|signal=<n> then the child's stderr tail (Rosetta's fatal text).
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <signal.h>
#include <unistd.h>
#include <sys/mman.h>
#include <sys/wait.h>
#include <stdint.h>
#include <cpuid.h>
#include <fcntl.h>

static unsigned char *pg;           // 2 pages; case works at pg+4096-16 (crossing) or pg
static size_t PS = 16384;           // arm64 page size (Rosetta guest sees 4096 but host faults at 16K)
static int faults;
static void on_segv(int s, siginfo_t *si, void *u) {
    (void)s; (void)u; faults++;
    uintptr_t a = (uintptr_t)si->si_addr & ~(uintptr_t)(PS - 1);
    mprotect((void *)a, PS, PROT_READ | PROT_WRITE);
}
typedef struct { const char *name; void (*f)(unsigned char *); } Case;
#define A(...) __asm__ volatile(__VA_ARGS__)
static void c_mov(unsigned char *p){ A("movl $0x11223344, (%0)"::"r"(p)); }
static void c_ymm_store(unsigned char *p){ A("vmovdqu %%ymm0,(%0)"::"r"(p):"ymm0"); }
static void c_xmm_store(unsigned char *p){ A("vmovdqu %%xmm0,(%0)"::"r"(p):"xmm0"); }
static void c_vpmaskmovd_full(unsigned char *p){ A("vpcmpeqd %%ymm1,%%ymm1,%%ymm1; vpmaskmovd %%ymm0,%%ymm1,(%0)"::"r"(p):"ymm0","ymm1"); }
static void c_vpmaskmovd_part(unsigned char *p){ A("vpxor %%ymm1,%%ymm1,%%ymm1; mov $-1,%%eax; vmovd %%eax,%%xmm2; vpblendd $1,%%ymm2,%%ymm1,%%ymm1; vpmaskmovd %%ymm0,%%ymm1,(%0)"::"r"(p):"rax","ymm0","ymm1","ymm2"); }
static void c_vpmaskmovd_zero(unsigned char *p){ A("vpxor %%ymm1,%%ymm1,%%ymm1; vpmaskmovd %%ymm0,%%ymm1,(%0)"::"r"(p):"ymm0","ymm1"); }
static void c_vpmaskmovq_full(unsigned char *p){ A("vpcmpeqd %%ymm1,%%ymm1,%%ymm1; vpmaskmovq %%ymm0,%%ymm1,(%0)"::"r"(p):"ymm0","ymm1"); }
static void c_vmaskmovps_full(unsigned char *p){ A("vpcmpeqd %%ymm1,%%ymm1,%%ymm1; vmaskmovps %%ymm0,%%ymm1,(%0)"::"r"(p):"ymm0","ymm1"); }
static void c_vmaskmovpd_full(unsigned char *p){ A("vpcmpeqd %%ymm1,%%ymm1,%%ymm1; vmaskmovpd %%ymm0,%%ymm1,(%0)"::"r"(p):"ymm0","ymm1"); }
static void c_vpmaskmovd_xmm(unsigned char *p){ A("vpcmpeqd %%xmm1,%%xmm1,%%xmm1; vpmaskmovd %%xmm0,%%xmm1,(%0)"::"r"(p):"xmm0","xmm1"); }
static void c_vcvtps2ph_mem(unsigned char *p){ A("vcvtps2ph $0,%%ymm0,(%0)"::"r"(p):"ymm0"); }
static void c_vextractf128_mem(unsigned char *p){ A("vextractf128 $1,%%ymm0,(%0)"::"r"(p):"ymm0"); }
static void c_movnti(unsigned char *p){ A("movnti %%eax,(%0)"::"r"(p):"rax"); }
static void c_vmovntdq(unsigned char *p){ A("vmovntdq %%ymm0,(%0)"::"r"(p):"ymm0"); }
static void c_movntdq(unsigned char *p){ A("movntdq %%xmm0,(%0)"::"r"(p):"xmm0"); }
static void c_clwb(unsigned char *p){ A(".byte 0x66,0x0f,0xae,0x30"::"a"(p):"cc"); }          // clwb (%rax)
static void c_clflushopt(unsigned char *p){ A(".byte 0x66,0x0f,0xae,0x38"::"a"(p):"cc"); }    // clflushopt (%rax)
static void c_clflush(unsigned char *p){ A("clflush (%0)"::"r"(p)); }
static void c_lock_cmpxchg(unsigned char *p){ uint32_t e=0; A("lock cmpxchgl %%edx,(%0)"::"r"(p),"a"(e),"d"(1u):"cc"); }
static void c_lock_xadd(unsigned char *p){ A("lock xaddl %%edx,(%0)"::"r"(p),"d"(1u):"cc"); }
static void c_lock_or(unsigned char *p){ A("lock orl $1,(%0)"::"r"(p):"cc"); }
static void c_lock_bts(unsigned char *p){ A("lock btsl $3,(%0)"::"r"(p):"cc"); }
static void c_xchg(unsigned char *p){ A("xchgl %%edx,(%0)"::"r"(p),"d"(1u)); }
static void c_cmpxchg16b(unsigned char *p){ A("lock cmpxchg16b (%0)"::"r"((uintptr_t)p&~15ull),"a"(0),"d"(0),"b"(1),"c"(0):"cc"); }
static void c_rep_movs(unsigned char *p){ unsigned char s[64]={0}; void*d=p;void*sp=s;size_t n=64; A("rep movsb":"+D"(d),"+S"(sp),"+c"(n)); }
static void c_prefetch(unsigned char *p){ A("prefetcht0 (%0)"::"r"(p)); }

// non-store instructions: run in fault-free mode only
static unsigned long flags_after_lock_cmpxchg(int hit){
    uint32_t m = hit ? 0 : 5, ax = 0; unsigned long f;
    A("lock cmpxchgl %%edx,%1; pushfq; popq %0":"=r"(f),"+m"(m),"+a"(ax):"d"(1u):"cc");
    return f;
}
static void nonfault(const char *name){
    if(!strcmp(name,"rdtsc")){ uint32_t lo,hi; A("rdtsc":"=a"(lo),"=d"(hi)); printf("rdtsc lo=%u (nonzero=%d)\n",lo,(lo|hi)!=0); }
    else if(!strcmp(name,"rdtscp")){ uint32_t lo,hi,aux; A("rdtscp":"=a"(lo),"=d"(hi),"=c"(aux)); printf("rdtscp ok aux=%u\n",aux); }
    else if(!strcmp(name,"xgetbv")){ uint32_t lo,hi; A("xgetbv":"=a"(lo),"=d"(hi):"c"(0)); printf("xgetbv xcr0=0x%x%08x\n",hi,lo); }
    else if(!strcmp(name,"cpuid")){
        unsigned a,b,c,d; __cpuid(1,a,b,c,d);
        printf("cpuid1 ecx=0x%x (fma=%d osxsave=%d avx=%d f16c=%d popcnt=%d) edx=0x%x\n",c,!!(c&(1<<12)),!!(c&(1<<27)),!!(c&(1<<28)),!!(c&(1<<29)),!!(c&(1<<23)),d);
        __cpuid_count(7,0,a,b,c,d);
        printf("cpuid7 ebx=0x%x (avx2=%d bmi1=%d bmi2=%d erms=%d avx512f=%d clflushopt=%d clwb=%d sha=%d) ecx=0x%x\n",b,!!(b&(1<<5)),!!(b&(1<<3)),!!(b&(1<<8)),!!(b&(1<<9)),!!(b&(1<<16)),!!(b&(1<<23)),!!(b&(1<<24)),!!(b&(1<<29)),c);
        __cpuid(0x80000001,a,b,c,d);
        printf("cpuid80000001 ecx=0x%x (lzcnt=%d) \n",c,!!(c&(1<<5)));
        char v[13]; __cpuid(0,a,b,c,d); memcpy(v,&b,4);memcpy(v+4,&d,4);memcpy(v+8,&c,4);v[12]=0; printf("vendor=%s maxleaf=%u\n",v,a);
    }
    else if(!strcmp(name,"lock_cmpxchg_flags")){ printf("lock cmpxchg [m],edx flags hit=0x%lx miss=0x%lx\n",flags_after_lock_cmpxchg(1)&0xcff,flags_after_lock_cmpxchg(0)&0xcff); }
    else if(!strcmp(name,"bmi")){ uint32_t r; A("andn %2,%1,%0":"=r"(r):"r"(5u),"r"(7u)); uint32_t t; A("tzcnt %1,%0":"=r"(t):"r"(8u)); uint32_t b; A("bzhi %2,%1,%0":"=r"(b):"r"(0xffu),"r"(4u)); uint32_t px; A("pext %2,%1,%0":"=r"(px):"r"(0xf0u),"r"(0xf0u)); uint64_t hi,lo; A("mulx %3,%1,%0":"=r"(hi),"=r"(lo):"d"(3ull),"r"(5ull)); printf("bmi andn=%u tzcnt=%u bzhi=%u pext=%u mulx=%llu\n",r,t,b,px,(unsigned long long)lo); }
    else if(!strcmp(name,"fma")){ float o[8]; A("vxorps %%ymm0,%%ymm0,%%ymm0; vbroadcastss %1,%%ymm1; vfmadd231ps %%ymm1,%%ymm1,%%ymm0; vmovups %%ymm0,%0":"=m"(o):"m"(*(float[]){3.0f}):"ymm0","ymm1"); printf("fma %f\n",o[0]); }
    else if(!strcmp(name,"f16c")){ float in[8]={1,2,3,4,5,6,7,8},o[8]; uint16_t h[8]; A("vmovups %1,%%ymm0; vcvtps2ph $0,%%ymm0,%%xmm1; vcvtph2ps %%xmm1,%%ymm2; vmovups %%ymm2,%0; vmovdqu %%xmm1,%2":"=m"(o),"=m"(h):"m"(in)); }
    else if(!strcmp(name,"gather")){ int32_t t[16]; for(int i=0;i<16;i++)t[i]=i*3; int32_t o[8]; A("vmovdqa %2,%%ymm1; vpcmpeqd %%ymm2,%%ymm2,%%ymm2; vpxor %%ymm0,%%ymm0,%%ymm0; vpgatherdd %%ymm2,(%1,%%ymm1,4),%%ymm0; vmovdqu %%ymm0,%0":"=m"(o):"r"(t),"m"(*(int32_t[]){0,1,2,3,4,5,6,7}):"ymm0","ymm1","ymm2"); printf("gather o[7]=%d\n",o[7]); }
    else if(!strcmp(name,"avx512")){ A("vpxord %%zmm0,%%zmm0,%%zmm0":::"zmm0"); printf("avx512 executed\n"); }
    else if(!strcmp(name,"xsave")){ static char buf[4096] __attribute__((aligned(64))); A("xsave (%0)"::"r"(buf),"a"(7),"d"(0)); printf("xsave ok\n"); }
    else if(!strcmp(name,"rdrand")){ unsigned r; unsigned char ok; A("rdrand %0; setc %1":"=r"(r),"=qm"(ok)); printf("rdrand ok=%d\n",ok); }
    else if(!strcmp(name,"popcnt_lzcnt")){ uint32_t a,b; A("popcnt %1,%0":"=r"(a):"r"(0xffu)); A("lzcnt %1,%0":"=r"(b):"r"(1u)); printf("popcnt=%u lzcnt=%u\n",a,b); }
    else if(!strcmp(name,"clzero")){ A(".byte 0x0f,0x01,0xfc"::"a"((uintptr_t)pg & ~63ull)); printf("clzero executed\n"); }
    else if(!strcmp(name,"monitorx")){ A(".byte 0x0f,0x01,0xfa"::"a"(pg),"c"(0),"d"(0)); printf("monitorx executed\n"); }
    else if(!strcmp(name,"pause_mfence")){ A("pause; mfence; sfence; lfence"); printf("pause/fences ok\n"); }
    else { printf("unknown\n"); }
}
static Case cases[] = {
 {"mov32",c_mov},{"vmovdqu_ymm_store",c_ymm_store},{"vmovdqu_xmm_store",c_xmm_store},
 {"vpmaskmovd_ymm_full",c_vpmaskmovd_full},{"vpmaskmovd_ymm_lane0",c_vpmaskmovd_part},{"vpmaskmovd_ymm_zero_mask",c_vpmaskmovd_zero},
 {"vpmaskmovq_ymm_full",c_vpmaskmovq_full},{"vmaskmovps_ymm_full",c_vmaskmovps_full},{"vmaskmovpd_ymm_full",c_vmaskmovpd_full},{"vpmaskmovd_xmm_full",c_vpmaskmovd_xmm},
 {"vcvtps2ph_mem",c_vcvtps2ph_mem},{"vextractf128_mem",c_vextractf128_mem},
 {"movnti",c_movnti},{"vmovntdq_ymm",c_vmovntdq},{"movntdq_xmm",c_movntdq},
 {"clwb",c_clwb},{"clflushopt",c_clflushopt},{"clflush",c_clflush},
 {"lock_cmpxchg",c_lock_cmpxchg},{"lock_xadd",c_lock_xadd},{"lock_or",c_lock_or},{"lock_bts",c_lock_bts},{"xchg_mem",c_xchg},{"lock_cmpxchg16b",c_cmpxchg16b},
 {"rep_movsb",c_rep_movs},{"prefetcht0",c_prefetch},
};
int main(int argc,char**argv){
    if(argc<2){ printf("usage: probe list | <case> <inpage|cross|prot_target> | nonfault <name>\n"); for(unsigned i=0;i<sizeof cases/sizeof*cases;i++)printf("%s\n",cases[i].name); return 0; }
    if(!strcmp(argv[1],"list")){ for(unsigned i=0;i<sizeof cases/sizeof*cases;i++)printf("%s\n",cases[i].name); return 0; }
    pg = mmap(0, 4*PS, PROT_READ|PROT_WRITE, MAP_ANON|MAP_PRIVATE, -1, 0);
    if(!strcmp(argv[1],"nonfault")){ nonfault(argv[2]); return 0; }
    struct sigaction sa; memset(&sa,0,sizeof sa); sa.sa_sigaction=on_segv; sa.sa_flags=SA_SIGINFO|SA_NODEFER; sigaction(SIGSEGV,&sa,0); sigaction(SIGBUS,&sa,0);
    const char *mode = argc>2?argv[2]:"inpage";
    for(unsigned i=0;i<sizeof cases/sizeof*cases;i++) if(!strcmp(cases[i].name,argv[1])){
        unsigned char *p = pg + PS;              // 2nd page begins here
        if(!strcmp(mode,"inpage")){ p = pg+64; }                              // no fault
        else if(!strcmp(mode,"fault")){ mprotect(pg+PS,PS,PROT_NONE); }       // destination page inaccessible
        else if(!strcmp(mode,"cross")){ mprotect(pg+PS,PS,PROT_NONE); p = pg+PS-16; } // 32-byte store: 1st half ok, 2nd in guarded page
        else if(!strcmp(mode,"ro")){ mprotect(pg+PS,PS,PROT_READ); }          // read-only (write watch style)
        cases[i].f(p);
        printf("%s %s done faults=%d\n",argv[1],mode,faults); return 0; }
    printf("no such case\n"); return 2;
}
