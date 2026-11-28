// Independently check the production mask model on arm64, exhausting lane masks.
#include "../isa-model-masked.h"
int main(void) {
    long n=0,bad=0;
    for(int w=16;w<=32;w*=2) for(int e=4;e<=8;e*=2) for(int off=0;off<=3;off+=3) for(int store=0;store<2;store++) {
        if(store && w+off>32)continue;
        for(int bits=0;bits<(1<<(w/e));bits++) {
            IRec r={0};Out o={0};Op op={0};op.memoff=off;op.p=(void*)(intptr_t)(e+(store<<8));Ctx c={.op=&op,.w=w};unsigned char want[32];
            for(int j=0;j<32;j++){r.b[j]=(unsigned char)(j*11+3);r.c[j]=(unsigned char)(j*7+11);o.r[j]=(unsigned char)(101+j);want[j]=store?o.r[j]:0;r.a[j]=0x7f;}
            for(int lane=0;lane<w/e;lane++)if((bits>>lane)&1){r.a[(lane+1)*e-1]=0x80;for(int b=0;b<e;b++){int j=lane*e+b;if(store)want[j+off]=r.b[j];else want[j]=j+off<32?r.b[j+off]:r.c[j+off-32];}}
            m_mask(&c,&r,&o);bad+=memcmp(want,o.r,32)!=0;n++;
        }
    }
    printf("validate-masked: %ld masks/forms, %ld mismatches\n",n,bad);return bad!=0;
}
