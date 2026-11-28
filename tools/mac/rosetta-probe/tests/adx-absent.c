// ADX availability: CPUID.(EAX=7,ECX=0):EBX[19] and what adcx / adox do (an architecturally correct CPU that does not advertise ADX raises #UD).
#include <windows.h>
#include <stdio.h>
#include <intrin.h>
#include <setjmp.h>
static jmp_buf jb; static volatile int got;
static LONG CALLBACK veh(PEXCEPTION_POINTERS p) { got = (int)p->ExceptionRecord->ExceptionCode; longjmp(jb, 1); }
int main(void) {
    int r[4]; __cpuidex(r, 7, 0); printf("CPUID.7.0:EBX[19] (ADX) = %d\n", (r[1] >> 19) & 1);
    AddVectoredExceptionHandler(1, veh);
    got = 0; if (!setjmp(jb)) { unsigned long long a = 1, b = 2; __asm__ volatile("stc\n adcx %1,%0" : "+r"(a) : "r"(b) : "cc"); printf("adcx executed, a=%llu\n", a); } else printf("adcx raised exception %08x%s\n", got, got == (int)EXCEPTION_ILLEGAL_INSTRUCTION ? " (illegal instruction = #UD)" : "");
    got = 0; if (!setjmp(jb)) { unsigned long long a = 1, b = 2; __asm__ volatile("adox %1,%0" : "+r"(a) : "r"(b) : "cc"); printf("adox executed, a=%llu\n", a); } else printf("adox raised exception %08x%s\n", got, got == (int)EXCEPTION_ILLEGAL_INSTRUCTION ? " (illegal instruction = #UD)" : "");
    return 0;
}
