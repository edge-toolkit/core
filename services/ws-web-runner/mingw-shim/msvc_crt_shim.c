/* Shims for the MSVC static-CRT symbols the msvc-built rusty_v8 static lib needs but no runtime DLL exports.
 * On x86_64-pc-windows-gnu the prebuilt rusty_v8 archive is the MSVC one (upstream ships no gnu
 * build; see build.rs), and MSVC normally links these from its static CRT pieces (msvcrt.lib /
 * libvcruntime.lib) -- mingw-w64 has no equivalent, so this file supplies them. Symbols exported by
 * vcruntime140.dll / ucrtbase.dll are NOT shimmed; build.rs links winlibs' import libs for those.
 * MSVC-mangled `??...` names live in msvc_crt_ops.s (GAS only accepts `?` in quoted symbols), jumping to
 * the plain-named impls below -- MSVC x64 and mingw x64 share the Microsoft x64 calling convention. */

#include <stdint.h>
#include <stdlib.h>
#include <windows.h>

/* MSVC's "floating point used" CRT marker; the value is what MSVC's own CRT sets.
 * mingw-w64 keeps its copy in a dedicated archive member that this force-linked object shadows without
 * collision. */
int _fltused = 0x9875;

/* winlibs' libucrtbase.a import lib lacks _strtold_l.
 * MSVC long double IS double (both return in xmm0 with identical argument registers), so forward to the
 * _strtod_l import untouched. */
__attribute__((naked)) void _strtold_l(void) { __asm__("jmp _strtod_l"); }

/* /GS stack cookie (libvcruntime static). Fixed default cookie; the check is a no-op. */
uintptr_t __security_cookie = 0x00002B992DDFA232ULL;
__attribute__((naked)) void __security_check_cookie(void) { __asm__("ret"); }

/* MSVC stack probe: same contract as libgcc's ___chkstk_ms (rax = frame size, all regs preserved). */
__attribute__((naked)) void __chkstk(void) { __asm__("jmp ___chkstk_ms"); }

/* Control Flow Guard dispatch/check pointers, with the non-CFG default behaviour.
 * Guarded code does `call *__guard_dispatch_icall_fptr` with the target in rax; the defaults are dispatch
 * jumps to rax, check returns. */
__attribute__((naked)) static void guard_dispatch_impl(void) { __asm__("jmp *%rax"); }
__attribute__((naked)) static void guard_check_impl(void) { __asm__("ret"); }
void (*__guard_dispatch_icall_fptr)(void) = guard_dispatch_impl;
void (*__guard_check_icall_fptr)(void) = guard_check_impl;

/* Thread-safe static init (vcruntime's thread_safe_statics.cpp contract).
 * Call-site codegen:
 *   if (*g > _Init_thread_epoch) { header(g); if (*g == -1) { <init>; footer(g); } }
 * with guard values 0 = uninitialized, -1 = in progress, else = completion epoch. The per-thread
 * _Init_thread_epoch TLS int lives in msvc_crt_ops.s (a real .tls$ symbol; a gcc __thread variable would
 * be emutls, which MSVC SECREL relocations can't bind to) and stays at INT_MIN forever: after init the
 * call site then re-enters header on every access, which just reads the completed guard and returns.
 * Slower than MSVC's epoch fast-path but correct. */
void _Init_thread_header(volatile int *g) {
    static SRWLOCK lock = SRWLOCK_INIT;
    AcquireSRWLockExclusive(&lock);
    for (;;) {
        if (*g == 0) {
            *g = -1;
            break;
        } /* claim: caller runs the initializer */
        if (*g != -1)
            break;                      /* completed by another thread */
        ReleaseSRWLockExclusive(&lock); /* in progress elsewhere: spin politely */
        Sleep(0);
        AcquireSRWLockExclusive(&lock);
    }
    ReleaseSRWLockExclusive(&lock);
}

void _Init_thread_footer(volatile int *g) { *g = 1; }

void _Init_thread_abort(volatile int *g) { *g = 0; }

/* MSVC dynamic-TLS on-demand hook.
 * The __tls_guard TLS byte (msvc_crt_ops.s) is pre-set to 1, so MSVC call sites skip this; the mingw-w64
 * crt's TLS callback already walks the .CRT$XD* initializer table. */
void __dyn_tls_on_demand_init(void) {}

/* Chromium libc++'s verbose abort, which exists only to die loudly.
 * It is referenced from the archive's absl objects but its own definition is not archive-extractable. */
_Noreturn void shim_libcpp_verbose_abort(const char *fmt, ...) {
    (void)fmt;
    abort();
}

/* MSVC C++ operator new/delete impls (statically linked in MSVC's CRT), forwarded to the mingw heap.
 * V8 frees what it allocates, so pairing stays within one heap. Throwing-new degrades to abort-on-OOM. */
void *shim_op_new(size_t n) {
    void *p = malloc(n ? n : 1);
    if (!p)
        abort();
    return p;
}
void *shim_op_new_nothrow(size_t n, void *tag) {
    (void)tag;
    return malloc(n ? n : 1);
}
void *shim_op_new_aligned(size_t n, size_t align) {
    void *p = _aligned_malloc(n ? n : 1, align);
    if (!p)
        abort();
    return p;
}
void shim_op_delete(void *p) { free(p); }
void shim_op_delete_sized(void *p, size_t n) {
    (void)n;
    free(p);
}
void shim_op_delete_aligned(void *p, size_t align) {
    (void)align;
    _aligned_free(p);
}
void shim_op_delete_sized_aligned(void *p, size_t n, size_t align) {
    (void)n;
    (void)align;
    _aligned_free(p);
}
