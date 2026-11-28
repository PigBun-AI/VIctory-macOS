/* Does a thread keep its data when the view it uses is unmapped and mapped again?  (mac.hle.xmem_map_partial_view)
 *
 * Windows build (llvm-mingw), run under Wine:   view-replace.exe [iterations] [mode] [wait]
 * Native macOS build (clang, -DNATIVE):         view-replace   [iterations]     (control: mmap instead of Wine)
 *
 * A section of 4 pages (64 KiB) is mapped as one view over a placeholder. Thread T counts in the first page: it reads
 * what it wrote last and writes one more. The main thread then does what victory-hle's XMemMapPhysicalPages does to
 * unmap the last page of a view: the whole view is unmapped (a placeholder again), the rest of it mapped again, the page
 * mapped again; three times (pages 3, 2, 1). T never gave up its page. A vectored handler (Windows build) waits until the
 * address is mapped read-write and resumes T, as victory-gpu's does (`came_back`); `wait` = 1: until the whole change is over.
 *
 * mode 0: as the HLE does.  mode 1: one view per page, only the page's own view is unmapped (the console's semantics): control.
 * mode 2: the whole view unmapped and mapped whole again.
 * Output: "BAD expected N got M; truth ..." per iteration in which T read other than it wrote (truth: a second view of the
 * whole section; "later": T's reads 2 ms after), then "done iters=.. bad=.. faults=..".
 * Result on this Mac (CrossOver Wine 26.3 on Rosetta): modes 0 and 2 fail about 1 in 500 iterations under load, mode 1 never;
 * the native control, arm64 and x86_64 under Rosetta without Wine, never.
 */
#include <stdio.h>
#include <stdlib.h>
#include <stdint.h>
#ifdef NATIVE
#include <signal.h>
#include <pthread.h>
#include <unistd.h>
#include <fcntl.h>
#include <sys/mman.h>
#include <stdatomic.h>
#define PAGE 65536
static char *ph; static int fd; static volatile int stop; static atomic_int dancing; static volatile uint64_t *truth;
static void on_fault(int s, siginfo_t *i, void *c) { while (atomic_load(&dancing)) ; }
static void *title(void *arg) {
    volatile uint64_t *p = (volatile uint64_t *)ph; uint64_t n = 0;
    while (!stop) {
        uint64_t got = *p;
        if (got != n) { printf("BAD expected %llu got %llu; truth %llu\n", (unsigned long long)n, (unsigned long long)got, (unsigned long long)*truth); return (void *)1; }
        n++; *p = n;
    }
    return 0;
}
int main(int argc, char **argv) {
    int iters = argc > 1 ? atoi(argv[1]) : 2000, bad = 0;
    struct sigaction sa = {0}; sa.sa_sigaction = on_fault; sa.sa_flags = SA_SIGINFO | SA_NODEFER;
    sigaction(SIGSEGV, &sa, 0); sigaction(SIGBUS, &sa, 0);
    char path[] = "/tmp/view-replace-XXXXXX"; fd = mkstemp(path); unlink(path); ftruncate(fd, 4 * PAGE);
    truth = mmap(0, 4 * PAGE, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    for (int it = 0; it < iters; it++) {
        stop = 0;
        ph = mmap(0, 4 * PAGE, PROT_NONE, MAP_PRIVATE | MAP_ANON, -1, 0);
        mmap(ph, 4 * PAGE, PROT_READ | PROT_WRITE, MAP_SHARED | MAP_FIXED, fd, 0);
        *(volatile uint64_t *)ph = 0;
        pthread_t t; pthread_create(&t, 0, title, 0);
        while (*(volatile uint64_t *)ph < 100) ;
        for (int page = 3; page >= 1; page--) {
            atomic_store(&dancing, 1);
            mmap(ph, 4 * PAGE, PROT_NONE, MAP_PRIVATE | MAP_ANON | MAP_FIXED, -1, 0);
            mmap(ph, 4 * PAGE, PROT_READ | PROT_WRITE, MAP_SHARED | MAP_FIXED, fd, 0);
            atomic_store(&dancing, 0);
        }
        stop = 1; void *r; pthread_join(t, &r); if (r) bad++;
        munmap(ph, 4 * PAGE);
    }
    printf("native done iters=%d bad=%d\n", iters, bad);
    return bad != 0;
}
#else
#include <windows.h>
#define PAGE 65536
typedef PVOID (WINAPI *PMapViewOfFile3)(HANDLE, HANDLE, PVOID, ULONG64, SIZE_T, ULONG, ULONG, MEM_EXTENDED_PARAMETER *, ULONG);
typedef PVOID (WINAPI *PVirtualAlloc2)(HANDLE, PVOID, SIZE_T, ULONG, ULONG, MEM_EXTENDED_PARAMETER *, ULONG);
typedef BOOL (WINAPI *PUnmapViewOfFile2)(HANDLE, PVOID, ULONG);
static PMapViewOfFile3 MapView3; static PVirtualAlloc2 VAlloc2; static PUnmapViewOfFile2 Unmap2;
static volatile LONG stop, faults, dancing;
static int wait_dance;
static char *ph, *truthbase; static HANDLE sec;
static LONG CALLBACK veh(EXCEPTION_POINTERS *p) {
    if (p->ExceptionRecord->ExceptionCode != EXCEPTION_ACCESS_VIOLATION) return EXCEPTION_CONTINUE_SEARCH;
    void *at = (void *)p->ExceptionRecord->ExceptionInformation[1];
    InterlockedIncrement(&faults);
    if (wait_dance) while (dancing) YieldProcessor();
    for (int i = 0; i < 2000000; i++) {
        MEMORY_BASIC_INFORMATION mbi;
        if (VirtualQuery(at, &mbi, sizeof mbi) && mbi.State == MEM_COMMIT && mbi.Type == MEM_MAPPED && mbi.Protect == PAGE_READWRITE)
            return EXCEPTION_CONTINUE_EXECUTION;
        YieldProcessor();
    }
    return EXCEPTION_CONTINUE_SEARCH;
}
static DWORD WINAPI title(void *arg) {
    volatile uint64_t *p = (volatile uint64_t *)ph, *tp = (volatile uint64_t *)truthbase; uint64_t n = 0;
    while (!stop) {
        uint64_t got = *p;
        if (got != n) {
            uint64_t t0 = *tp, again[4];
            for (int q = 0; q < 4; q++) { Sleep(2); again[q] = *p; }
            printf("BAD expected %llu got %llu; truth(whole-section view) %llu; later %llu %llu %llu %llu\n", (unsigned long long)n, (unsigned long long)got,
                   (unsigned long long)t0, (unsigned long long)again[0], (unsigned long long)again[1], (unsigned long long)again[2], (unsigned long long)again[3]);
            fflush(stdout);
            return 1;
        }
        n++; *p = n;
    }
    return 0;
}
static void viewmap(char *at, ULONG64 off, SIZE_T len) {
    void *v = MapView3(sec, GetCurrentProcess(), at, off, len, MEM_REPLACE_PLACEHOLDER, PAGE_READWRITE, NULL, 0);
    if (!v) { VirtualFree(at, len, MEM_RELEASE | MEM_PRESERVE_PLACEHOLDER); v = MapView3(sec, GetCurrentProcess(), at, off, len, MEM_REPLACE_PLACEHOLDER, PAGE_READWRITE, NULL, 0); }
    if (!v) { printf("map failed %lu\n", GetLastError()); exit(4); }
}
int main(int argc, char **argv) {
    HMODULE k = LoadLibraryA("kernelbase.dll");
    MapView3 = (PMapViewOfFile3)GetProcAddress(k, "MapViewOfFile3");
    VAlloc2 = (PVirtualAlloc2)GetProcAddress(k, "VirtualAlloc2");
    Unmap2 = (PUnmapViewOfFile2)GetProcAddress(k, "UnmapViewOfFile2");
    if (!MapView3 || !VAlloc2 || !Unmap2) { printf("no placeholder API\n"); return 2; }
    int iters = argc > 1 ? atoi(argv[1]) : 2000, mode = argc > 2 ? atoi(argv[2]) : 0, bad = 0;
    wait_dance = argc > 3 ? atoi(argv[3]) : 0;
    AddVectoredExceptionHandler(1, veh);
    sec = CreateFileMappingA(INVALID_HANDLE_VALUE, NULL, PAGE_READWRITE, 0, 4 * PAGE, NULL);
    truthbase = MapViewOfFile(sec, FILE_MAP_WRITE, 0, 0, 0);
    for (int it = 0; it < iters; it++) {
        stop = 0;
        ph = VAlloc2(GetCurrentProcess(), NULL, 4 * PAGE, MEM_RESERVE | MEM_RESERVE_PLACEHOLDER, PAGE_NOACCESS, NULL, 0);
        if (!ph) { printf("placeholder failed\n"); return 3; }
        if (mode != 1) viewmap(ph, 0, 4 * PAGE); else for (int q = 0; q < 4; q++) viewmap(ph + q * PAGE, (ULONG64)q * PAGE, PAGE);
        *(volatile uint64_t *)ph = 0;
        HANDLE t = CreateThread(NULL, 0, title, NULL, 0, NULL);
        while (*(volatile uint64_t *)ph < 100) { DWORD ec; GetExitCodeThread(t, &ec); if (ec != STILL_ACTIVE) break; }
        for (int page = 3; page >= 1; page--) {
            char *at = ph + page * PAGE;
            InterlockedExchange(&dancing, 1);
            if (mode == 0) {
                Unmap2(GetCurrentProcess(), ph, MEM_PRESERVE_PLACEHOLDER);   /* the whole view that starts at ph */
                viewmap(ph, 0, page * PAGE);                                 /* what stays of it */
                viewmap(at, (ULONG64)page * PAGE, PAGE);                     /* the page mapped again */
            } else if (mode == 2) {
                Unmap2(GetCurrentProcess(), ph, MEM_PRESERVE_PLACEHOLDER);
                viewmap(ph, 0, 4 * PAGE);
            } else {
                Unmap2(GetCurrentProcess(), at, MEM_PRESERVE_PLACEHOLDER);
                viewmap(at, (ULONG64)page * PAGE, PAGE);
            }
            InterlockedExchange(&dancing, 0);
        }
        stop = 1;
        WaitForSingleObject(t, INFINITE);
        DWORD ec; GetExitCodeThread(t, &ec); if (ec) bad++;
    }
    printf("done iters=%d mode=%d wait=%d bad=%d faults=%ld\n", iters, mode, wait_dance, bad, faults);
    return bad != 0;
}
#endif
