/* Heap-allocation shims for the msvc rusty_v8 archive, split out of msvc_crt_shim.c / msvc_crt_locale.c.
 * This is the shim's only dynamic-allocation code: the operator new/delete impls forward C++ allocation to the
 * mingw heap, and _dupenv_s hands back a heap-owned copy of an environment variable. Both intrinsically call the
 * <stdlib.h> allocation functions MISRA 21.3 forbids -- you cannot implement an allocator without an allocator --
 * so this code is isolated here precisely so the one analyzer that cannot suppress that rule per line (Codacy's
 * cxx / cppcheck, which offers only path-level excludes) can exclude just this file while the rest of the shim
 * stays fully analyzed. It remains covered by DeepSource's clang-tidy and the repo's own clang-tidy / cpplint /
 * flawfinder. Like msvc_crt_locale.c it links as a standalone object so _dupenv_s intercepts -lmsvcrt, and the
 * operator-new symbols resolve the msvc_crt_ops.s jumps in the shim archive. */

#include <malloc.h>
#include <stddef.h>
#include <stdlib.h>
#include <windows.h>

/* _dupenv_s, reimplemented on the msvcrt heap so allocation and release agree.
 * The real one is ucrt-only, so it binds to ucrtbase and would allocate from the UCRT heap -- but the
 * archive frees the returned buffer with `free`, which is msvcrt-bound. errno values per the MSVC contract.
 * Reads the value with Win32 GetEnvironmentVariableA rather than stdlib getenv: getenv is untrusted-input-prone
 * (MISRA 21.8), whereas GetEnvironmentVariableA reports the length itself, so no strlen over-read (CWE-126). */
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
    /* First call sizes the value (return includes the NUL); 0 means the variable is unset. */
    DWORD size = GetEnvironmentVariableA(name, NULL, 0);
    if (size == 0U) {
        return 0;
    }
    char *out = malloc(size);
    if (out == NULL) {
        return 12; /* ENOMEM */
    }
    /* Second call fills the buffer; its return excludes the NUL.
     * A value >= size means the variable changed between the calls (another thread), so treat it as unset. */
    DWORD written = GetEnvironmentVariableA(name, out, size);
    if ((written == 0U) || (written >= size)) {
        free(out);
        return 0;
    }
    *buf = out;
    if (len != NULL) {
        *len = (size_t)size; /* buffer size incl NUL, matching the old strlen(value) + 1 */
    }
    return 0;
}

/* MSVC C++ operator new/delete impls (statically linked in MSVC's CRT), forwarded to the mingw heap.
 * V8 frees what it allocates, so pairing stays within one heap. Throwing-new degrades to abort-on-OOM. */
void *shim_op_new(size_t n) {
    void *p = malloc(n ? n : 1U);
    if (!p) {
        abort();
    }
    return p;
}
void *shim_op_new_nothrow(size_t n, void *tag) {
    (void)tag;
    return malloc(n ? n : 1U);
}
void *shim_op_new_aligned(size_t n, size_t align) {
    void *p = _aligned_malloc(n ? n : 1U, align);
    if (!p) {
        abort();
    }
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
// The (size, align) params match the MSVC sized/aligned operator-delete ABI, so their order is fixed here.
// `n` (the size) is unused, so the swap-risk warning does not apply.
// NOLINTNEXTLINE(bugprone-easily-swappable-parameters)
void shim_op_delete_sized_aligned(void *p, size_t n, size_t align) {
    (void)n;
    (void)align;
    _aligned_free(p);
}
