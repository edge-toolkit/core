#include <stddef.h>
#include <stdint.h>

// Returns the sum of all bytes in buf, mod 256.
uint8_t byte_sum(const uint8_t *buf, size_t len) {
    uint8_t acc = 0;
    for (size_t i = 0; i < len; i++) acc += buf[i];
    return acc;
}
