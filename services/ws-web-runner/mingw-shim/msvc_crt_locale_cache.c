/* Exported locale wrappers for the msvc rusty_v8 archive, split out of msvc_crt_locale.c.
 * These four symbols (setlocale, localeconv, _create_locale, _free_locale) are the strong definitions that must
 * intercept the names -lmsvcrt would otherwise satisfy first, so like msvc_crt_locale.c they link as a standalone
 * object on the link line, not an archive member -- a linker only extracts archive members left-to-right, so an
 * archived setlocale would lose to msvcrt's. Each wrapper caches its resolved ucrtbase target in a write-once
 * static populated race-free by ucrt_resolve_once. Those function-local statics are why this file is isolated:
 * Codacy's "Local static variable" rule flags them for audit and, unlike DeepSource (suppressed inline with
 * skipcq), offers only path-level excludes -- so .codacy.yaml excludes just this file while the rest of the shim,
 * including the ucrt_resolve_once / ucrt_sym resolver these call, stays fully analyzed. It also stays covered by
 * DeepSource's clang-tidy and the repo's own clang-tidy / cpplint / flawfinder. The caches are reviewed-safe:
 * each is written exactly once, to a resolved function address. */

// ucrt_resolve_once is defined in msvc_crt_locale.c; declared here so this standalone object links against it.
void *ucrt_resolve_once(void *volatile *cache, const char *name);

char *setlocale(int category, const char *locale) {
    // skipcq: CXX-W2009 -- write-once symbol cache
    static void *volatile fn;
    return ((char *(*)(int, const char *))ucrt_resolve_once(&fn, "setlocale"))(category, locale);
}

/* struct lconv stays opaque here; the msvc-built caller knows the ucrt layout. */
void *localeconv(void) {
    // skipcq: CXX-W2009 -- write-once symbol cache
    static void *volatile fn;
    return ((void *(*)(void))ucrt_resolve_once(&fn, "localeconv"))();
}

// NOLINTNEXTLINE(bugprone-reserved-identifier, cert-dcl37-c)
void *_create_locale(int category, const char *locale) {
    // skipcq: CXX-W2009 -- write-once symbol cache
    static void *volatile fn;
    return ((void *(*)(int, const char *))ucrt_resolve_once(&fn, "_create_locale"))(category, locale);
}

// NOLINTNEXTLINE(bugprone-reserved-identifier, cert-dcl37-c)
void _free_locale(void *locale) {
    // skipcq: CXX-W2009 -- write-once symbol cache
    static void *volatile fn;
    ((void (*)(void *))ucrt_resolve_once(&fn, "_free_locale"))(locale);
}
