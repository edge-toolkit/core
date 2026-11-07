// C++ counterpart to util.c, compiled by the same `zig build` via clang's C++ mode.
// The wasm32-freestanding target has no libc++ (and `zig build` links none), so this file is freestanding C++:
// compiler-provided C headers only, no exceptions/RTTI (build.zig passes -fno-exceptions -fno-rtti), no operator
// new, and no non-trivial static initializers. Language-level features -- templates, constexpr, namespaces --
// all work.
// The <cstddef>/<cstdint> these would normally prefer are libc++ headers, and this target links no libc++.
// Only <stddef.h>/<stdint.h> are compiler-provided (they sit in clang's own resource include dir), so taking
// the advice would fail the build outright rather than modernise it.
// skipcq: CXX-W2030 -- freestanding wasm32: no libc++, so the C++ spellings do not exist here
#include <stddef.h>
// skipcq: CXX-W2030 -- freestanding wasm32: no libc++, so the C++ spellings do not exist here
#include <stdint.h>

namespace {

// FNV-1a parameters, specialized per hash width.
template <typename T> struct Fnv1aParams;

template <> struct Fnv1aParams<uint32_t> {
    static constexpr uint32_t offset_basis = 0x811c9dc5U;
    static constexpr uint32_t prime = 0x01000193U;
};

// Returns the FNV-1a hash of buf.
template <typename T> constexpr T fnv1a(const uint8_t *buf, size_t len) {
    T acc = Fnv1aParams<T>::offset_basis;
    for (size_t i = 0; i < len; i++) {
        acc = (acc ^ buf[i]) * Fnv1aParams<T>::prime;
    }
    return acc;
}

static_assert(fnv1a<uint32_t>(nullptr, 0) == 0x811c9dc5U, "empty input must hash to the offset basis");

}  // namespace

extern "C" uint32_t fnv1a_hash(const uint8_t *buf, size_t len) { return fnv1a<uint32_t>(buf, len); }
