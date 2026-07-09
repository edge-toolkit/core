/* Locale-consistency redirects for the msvc rusty_v8 archive.
 * Compiled as a STANDALONE OBJECT passed directly on the link line (see build.rs) -- an archive member
 * would lose the left-to-right race: the archive's undefined `setlocale` would be satisfied by -lmsvcrt
 * before the linker ever reached a shim archive appended after it.
 *
 * Why: the archive's libc++ creates locale handles with `_create_locale` and hands them to ucrt-only
 * stdio (`__stdio_common_vsprintf`). With mingw's default libs first, `_create_locale`/`setlocale` bind
 * to msvcrt.dll while the stdio stays ucrtbase.dll -- ucrt then dereferences an msvcrt-shaped _locale_t
 * and crashes (observed: access violation inside ucrtbase locale-name lookup under
 * v8::internal::ComputeFlagListHash's ostream << double). Redirecting the locale FAMILY to ucrtbase
 * keeps handle producers and consumers on one CRT. The heap family (malloc/free/_strdup/_msize) is NOT
 * redirected: it is msvcrt-consistent already, and the strings this path strdups are heap-agnostic.
 * _configthreadlocale is also NOT redirected: winlibs' libmsvcrt.a carries a real implementation object
 * for it (lib64_libmsvcrt_extra_a-_configthreadlocale.o) that other pulls drag in, so a strong definition
 * here would be a duplicate-symbol error; leaving it msvcrt-bound only costs per-thread-locale enablement
 * on the ucrt side, not handle compatibility.
 *
 * Resolution happens lazily on first call: the archive's C++ dynamic initializers call _create_locale
 * during the crt's _initterm walk, BEFORE gcc .ctors run (a plain constructor left the pointers NULL
 * there -> call through 0 before main), and a .CRT$XCT-registered initializer is unreferenced data that
 * -Wl,--gc-sections discards. GetProcAddress rather than an import lib because ucrtbase's own import-lib
 * members define these same names and would collide. */

#include <stdlib.h>
#include <string.h>
#include <windows.h>

static void *ucrt_sym(const char *name) {
    static HMODULE ucrt;
    if (ucrt == NULL) {
        ucrt = LoadLibraryW(L"ucrtbase.dll");
        if (ucrt == NULL) {
            abort();
        }
    }
    void *sym = (void *)GetProcAddress(ucrt, name);
    if (sym == NULL) {
        abort();
    }
    return sym;
}

char *setlocale(int category, const char *locale) {
    static char *(*fn)(int, const char *);
    if (fn == NULL) {
        fn = (char *(*)(int, const char *))ucrt_sym("setlocale");
    }
    return fn(category, locale);
}

/* struct lconv stays opaque here; the msvc-built caller knows the ucrt layout. */
void *localeconv(void) {
    static void *(*fn)(void);
    if (fn == NULL) {
        fn = (void *(*)(void))ucrt_sym("localeconv");
    }
    return fn();
}

// NOLINTNEXTLINE(bugprone-reserved-identifier, cert-dcl37-c)
void *_create_locale(int category, const char *locale) {
    static void *(*fn)(int, const char *);
    if (fn == NULL) {
        fn = (void *(*)(int, const char *))ucrt_sym("_create_locale");
    }
    return fn(category, locale);
}

// NOLINTNEXTLINE(bugprone-reserved-identifier, cert-dcl37-c)
void _free_locale(void *locale) {
    static void (*fn)(void *);
    if (fn == NULL) {
        fn = (void (*)(void *))ucrt_sym("_free_locale");
    }
    fn(locale);
}

/* _dupenv_s, reimplemented on the msvcrt heap so allocation and release agree.
 * The real one is ucrt-only, so it binds to ucrtbase and would allocate from the UCRT heap -- but the
 * archive frees the returned buffer with `free`, which is msvcrt-bound. errno values per the MSVC
 * contract. */
// _dupenv_s is declared __declspec(dllimport) by ucrt's stdlib.h, but here we define it on the mingw heap.
// The redeclaration drops dllimport by design -- the archive links our definition, not an import thunk.
// NOLINTNEXTLINE(clang-diagnostic-inconsistent-dllimport)
int _dupenv_s(char **buf, size_t *len, const char *name) {
    if ((buf == NULL) || (name == NULL)) {
        return 22; /* EINVAL */
    }
    *buf = NULL;
    if (len != NULL) {
        *len = 0;
    }
    const char *value = getenv(name); /* flawfinder: ignore [_dupenv_s shim: reading the env var is the point] */
    if (value == NULL) {
        return 0;
    }
    *buf = _strdup(value);
    if (*buf == NULL) {
        return 12; /* ENOMEM */
    }
    if (len != NULL) {
        *len = strlen(value) + 1U;
    }
    return 0;
}
