// The suggested <cstddef>/<cstdint> are not C headers at all, so this C++-only rule cannot apply here.
// This translation unit is C, and the wasm32-freestanding target links no libc++ either, so taking the advice
// would fail to compile rather than modernise anything.
// skipcq: CXX-W2030 -- C, not C++: <cstddef> does not exist in this language
#include <stddef.h>
// skipcq: CXX-W2030 -- C, not C++: <cstdint> does not exist in this language
#include <stdint.h>

// Returns the sum of all bytes in buf, mod 256.
uint8_t byte_sum(const uint8_t *buf, size_t len) {
    uint8_t acc = 0;
    for (size_t i = 0; i < len; i++)
        acc += buf[i];
    return acc;
}
