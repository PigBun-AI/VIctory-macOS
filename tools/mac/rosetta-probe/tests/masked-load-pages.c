// Windows/Wine: result and masked-off memory suppression, independent of the store-fault continuation harness.
#include <windows.h>
#include <immintrin.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#define V128(x) _mm_storeu_si128((__m128i*)out,(x))
#define V256(x) _mm256_storeu_si256((__m256i*)out,(x))
__attribute__((noinline)) static void load(unsigned form,const unsigned char *p,const unsigned char *mask,unsigned char *out) {
    __m128i x=_mm_loadu_si128((const __m128i*)mask); __m256i y=_mm256_loadu_si256((const __m256i*)mask);
    switch(form){
    case 0: V128(_mm_maskload_epi32((const int*)p,x));break;
    case 1: V128(_mm_castps_si128(_mm_maskload_ps((const float*)p,x)));break;
    case 2: V128(_mm_maskload_epi64((const long long*)p,x));break;
    case 3: V128(_mm_castpd_si128(_mm_maskload_pd((const double*)p,x)));break;
    case 4: V256(_mm256_maskload_epi32((const int*)p,y));break;
    case 5: V256(_mm256_castps_si256(_mm256_maskload_ps((const float*)p,y)));break;
    case 6: V256(_mm256_maskload_epi64((const long long*)p,y));break;
    case 7: V256(_mm256_castpd_si256(_mm256_maskload_pd((const double*)p,y)));break;
    }
}
int main(void){
    SYSTEM_INFO info;GetSystemInfo(&info);size_t page=info.dwPageSize; const size_t align=65536;
    // Choose a 64-KiB aligned boundary (also a host 16-KiB boundary). Protect at least one whole allocation granule on each side.
    unsigned char *b=VirtualAlloc(NULL,3*align,MEM_RESERVE|MEM_COMMIT,PAGE_READWRITE);if(!b)return 2;
    for(size_t i=0;i<3*align;i++) b[i]=(unsigned char)(1+((i*13)%251));
    DWORD old; if(!VirtualProtect(b,align,PAGE_NOACCESS,&old)||!VirtualProtect(b+2*align,align,PAGE_NOACCESS,&old))return 2;
    int n=0,bad=0,neg=getenv("PROBE_NEG")!=NULL;
    for(unsigned form=0;form<8;form++){
        int w=form>=4?32:16,e=(form&2)?8:4,lanes=w/e;
        for(int side=0;side<2;side++)for(int off=-w;off<=w;off++)for(unsigned bits=0;bits<(1U<<lanes);bits++){
            unsigned char *p=b+(side?2:1)*align+off,mask[32]={0},expect[32]={0},out[32]={0};int valid=1;
            for(int k=0;k<lanes;k++)if(bits&(1U<<k)){
                if(p+e*k<b+align||p+e*(k+1)>b+2*align){valid=0;break;}
                mask[e*k+e-1]=0x80;memcpy(expect+e*k,p+e*k,e);
            }
            if(!valid)continue;
            load(form,p,mask,out);if(neg&&!n)expect[0]^=1;bad+=memcmp(expect,out,w)!=0;n++;
        }
    }
    VirtualFree(b,0,MEM_RELEASE);printf("masked-load-pages: guest page %zu, 8 forms, %d masks/addresses, %d mismatches\n",page,n,bad);return bad?1:0;
}
