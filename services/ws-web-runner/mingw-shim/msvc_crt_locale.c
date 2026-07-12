/* ucrtbase symbol-resolution helpers for the msvc rusty_v8 locale shim.
 * The exported locale wrappers (setlocale, localeconv, _create_locale, _free_locale) live in a sibling standalone
 * object and call ucrt_resolve_once here; this file is the shared resolver they build on.
 *
 * Why the redirect exists: the archive's libc++ creates locale handles with `_create_locale` and hands them to
 * ucrt-only stdio (`__stdio_common_vsprintf`). With mingw's default libs first, `_create_locale`/`setlocale` bind
 * to msvcrt.dll while the stdio stays ucrtbase.dll -- ucrt then dereferences an msvcrt-shaped _locale_t and
 * crashes (observed: access violation inside ucrtbase locale-name lookup under v8::internal::ComputeFlagListHash's
 * ostream << double). Redirecting the locale FAMILY to ucrtbase keeps handle producers and consumers on one CRT.
 * The heap family (malloc/free/_strdup/_msize) is NOT redirected: it is msvcrt-consistent already, and the strings
 * this path strdups are heap-agnostic. _configthreadlocale is also NOT redirected: winlibs' libmsvcrt.a carries a
 * real implementation object for it (lib64_libmsvcrt_extra_a-_configthreadlocale.o) that other pulls drag in, so a
 * strong definition would be a duplicate-symbol error; leaving it msvcrt-bound only costs per-thread-locale
 * enablement on the ucrt side, not handle compatibility.
 *
 * Resolution happens lazily on first call, not at load time: the archive's C++ dynamic initializers call
 * _create_locale during the crt's _initterm walk, BEFORE gcc .ctors run, so a plain constructor that resolved
 * these symbols would not have run yet (call through 0 before main), and a .CRT$XCT-registered initializer is
 * unreferenced data that -Wl,--gc-sections discards. ucrt_resolve_once caches each resolved symbol in a
 * caller-supplied write-once slot and ucrt_sym caches the ucrtbase.dll handle; both publish with an interlocked
 * compare-exchange, so concurrent first callers converge on a single value (the handle loser drops its extra
 * LoadLibraryW ref). Uses GetProcAddress rather than an import lib because ucrtbase's own import-lib members
 * define these same names and would collide. */

#include <stdlib.h>
#include <windows.h>

static void *ucrt_sym(const char *name) {
    static HMODULE ucrt;
    HMODULE mod = (HMODULE)InterlockedCompareExchangePointer((void *volatile *)&ucrt, NULL, NULL);
    if (mod == NULL) {
        HMODULE loaded = LoadLibraryW(L"ucrtbase.dll");
        if (loaded == NULL) {
            abort();
        }
        HMODULE prev = (HMODULE)InterlockedCompareExchangePointer((void *volatile *)&ucrt, loaded, NULL);
        mod = (prev == NULL) ? loaded : prev;
        if (prev != NULL) {
            FreeLibrary(loaded);
        }
    }
    void *sym = (void *)GetProcAddress(mod, name);
    if (sym == NULL) {
        abort();
    }
    return sym;
}

// Resolve `name` once, caching the result in `*cache` with a race-free interlocked publish.
// External linkage so the locale wrappers' standalone object can link against it.
void *ucrt_resolve_once(void *volatile *cache, const char *name) {
    void *fn = InterlockedCompareExchangePointer(cache, NULL, NULL);
    if (fn == NULL) {
        void *resolved = ucrt_sym(name);
        void *prev = InterlockedCompareExchangePointer(cache, resolved, NULL);
        fn = (prev == NULL) ? resolved : prev;
    }
    return fn;
}
