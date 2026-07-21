// Demonstrates C++ exceptions in the .cpp layer of this Zig wasm module, plus the minimal runtime they need.
// Probed against zig's clang on wasm32-freestanding (run under V8 via node; V8 has native wasm
// exception-handling support), there are four exception models to choose from per translation unit:
//
// - `-fno-exceptions` (zig-data1's util.cpp model): `throw` / `try` are compile errors. The safe default for
//   a TU that doesn't need exceptions -- misuse is diagnosed at compile time.
// - Default C++ flags (exceptions nominally on, but no wasm EH): a `throw` still lowers to `__cxa_throw`, but
//   wasm has no unwinder here, so clang discards every `catch` as dead code. A throw can then never be caught
//   -- code that LOOKS exception-correct silently is not. Never ship this model.
// - `-fwasm-exceptions -mexception-handling` (this TU's model): real wasm exception-handling instructions.
//   `catch (...)` compiles to a plain wasm catch of the C++ tag and needs only the four Itanium ABI entry
//   points and the referenced typeinfo symbol, all defined below. Throw and catch then genuinely work: a
//   throw caught in C++ stays in C++, and a throw nothing catches surfaces to the JS host as a catchable
//   `WebAssembly.Exception` (both probed under V8).
// - Typed catches (e.g. `catch (int32_t)`) additionally require LSDA type matching -- the libc++abi
//   personality routine plus libunwind's wasm context, neither of which exists on wasm32-freestanding.
//   Linking a typed catch against this runtime fails with, verbatim:
//       wasm-ld: error: exc.o: undefined symbol: __wasm_lpad_context
//       wasm-ld: error: exc.o: undefined symbol: _Unwind_CallPersonality
//
// House rules that follow: only `catch (...)` is available; an exception must never unwind through Zig frames
// (Zig has no C++ cleanup semantics and the exception would escape as a raw `WebAssembly.Exception`), so every
// extern "C" entry point in an exception-enabled TU catches everything it can throw and translates the failure
// to a status code at the boundary, as try_divide() does at the bottom of this file.
#include <stddef.h>
#include <stdint.h>

namespace {

// One static in-flight exception slot: no nested or rethrown exceptions, which the demo never needs.
// The mutable-global suppressions below match the mingw-shim's ABI globals: the Itanium ABI hands the runtime
// ownership of in-flight exception state, so it cannot be const and cannot live on a stack frame.
// skipcq: CXX-W2009 -- in-flight exception state owned by the runtime
// skipcq: CXX-W2066 -- freestanding wasm32 has no libc++, so std::array/<array> is unavailable; fixed ABI slot
// NOLINTNEXTLINE(cppcoreguidelines-avoid-non-const-global-variables)
alignas(16) unsigned char exception_slot[64];  // flawfinder: ignore -- __cxa_allocate_exception bounds-checks size
// Destructor of the in-flight exception object, captured at throw time and run by __cxa_end_catch.
// skipcq: CXX-W2009 -- in-flight exception state owned by the runtime
// NOLINTNEXTLINE(cppcoreguidelines-avoid-non-const-global-variables)
void (*pending_dtor)(void *) = nullptr;

}  // namespace

// Minimal Itanium C++ ABI exception runtime.
// clang lowers `throw x` to __cxa_allocate_exception + construct + __cxa_throw, and a landed catch handler to
// __cxa_begin_catch / __cxa_end_catch; these definitions are the whole runtime a `catch (...)`-only TU links
// against. The double-underscore names are mandated by the ABI (they must match what clang emits), so the
// reserved-identifier checks are suppressed per symbol, as on the mingw-shim's CRT symbols.
extern "C" {

// Returns storage for a to-be-thrown exception object of `size` bytes, or null when it cannot fit.
// The generated throw site does not null-check, so an oversized payload traps -- acceptable for this demo.
// NOLINTNEXTLINE(bugprone-reserved-identifier, cert-dcl37-c, cert-dcl51-cpp)
void *__cxa_allocate_exception(size_t size) {
    return size <= sizeof(exception_slot) ? static_cast<void *>(exception_slot) : nullptr;
}

// Records the payload destructor, then throws the wasm C++ exception tag (tag 0) carrying the payload pointer.
// The typeinfo is ignored: with `catch (...)` only, no type matching ever happens. The (thrown, tinfo) pair is
// the ABI's parameter order, so the swap-risk warning does not apply.
// NOLINTNEXTLINE(bugprone-reserved-identifier, cert-dcl37-c, cert-dcl51-cpp, bugprone-easily-swappable-parameters)
void __cxa_throw(void *thrown, void *tinfo, void (*dtor)(void *)) {
    (void)tinfo;
    pending_dtor = dtor;
    __builtin_wasm_throw(0, thrown);
}

// Returns the payload pointer the catch handler adjusts from.
// This runtime throws the payload pointer itself, so it passes through unchanged.
// NOLINTNEXTLINE(bugprone-reserved-identifier, cert-dcl37-c, cert-dcl51-cpp)
void *__cxa_begin_catch(void *thrown) { return thrown; }

// Destroys the caught exception object, ending its lifetime in the static slot.
// NOLINTNEXTLINE(bugprone-reserved-identifier, cert-dcl37-c, cert-dcl51-cpp)
void __cxa_end_catch(void) {
    if (pending_dtor != nullptr) {
        pending_dtor(exception_slot);
        pending_dtor = nullptr;
    }
}

// Address-only stand-in for libc++abi's `typeinfo for int`.
// Every `throw <int>` site references _ZTIi, but this runtime never dereferences it, so a dummy with the
// right symbol name satisfies the linker.
struct MinimalTypeInfo {
    const void *vtable;
    const char *name;
};
// NOLINTNEXTLINE(bugprone-reserved-identifier, cert-dcl37-c, cert-dcl51-cpp)
extern const MinimalTypeInfo int_type_info asm("_ZTIi");
const MinimalTypeInfo int_type_info = {nullptr, "i"};

}  // extern "C"

namespace {

// Throwing callee: the int payload is trivially destructible, so __cxa_throw records a null destructor.
int32_t checked_divide(int32_t num, int32_t den) {
    if (den == 0) {
        throw den;
    }
    return num / den;
}

}  // namespace

// Exception-safe boundary: returns the quotient, or -1 when checked_divide() throws.
// The catch-all is the house rule above -- nothing may unwind past an extern "C" entry point into the Zig
// caller.
extern "C" int32_t try_divide(int32_t num, int32_t den) {
    try {
        return checked_divide(num, den);
    } catch (...) {
        return -1;
    }
}
