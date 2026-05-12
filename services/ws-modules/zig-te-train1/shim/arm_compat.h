/*
 * Portable C replacements for ARM DSP/SIMD intrinsics.
 *
 * Force-included by emcc via "-include src/arm_compat.h".
 *
 * Key rules:
 *  1. Block CMSIS per-compiler headers (define their include guards up front).
 *  2. Provide every intrinsic TinyEngine kernels use as portable C.
 *  3. Typedef guards — tinyengine_function.h also defines q7_t / q15_t / q31_t;
 *     use __TINYENGINE_TYPES_DEFINED so neither file redefines the other.
 */
#pragma once

/* ── Block CMSIS compiler-specific headers ─────────────────────────────── */
#define __CMSIS_GCC_H
#define __CMSIS_ARMCC_H
#define __CMSIS_ARMCLANG_H
#define __CMSIS_ARMCLANG_M_H
#define __CMSIS_ICCARM_H

/* ── Standard headers ──────────────────────────────────────────────────── */
#include <stdint.h>
#include <string.h>
#include <stdlib.h>

/* ── Primitive types ───────────────────────────────────────────────────── */
/*
 * tinyengine_function.h independently defines q7_t / q8_t / q15_t / q31_t.
 * Guard them so whichever header is processed first wins; both use the same
 * underlying types so the result is always correct.
 */
#ifndef __TINYENGINE_TYPES_DEFINED
#define __TINYENGINE_TYPES_DEFINED
typedef int8_t   q7_t;
typedef uint8_t  q8_t;
typedef int16_t  q15_t;
typedef uint16_t q16_t;
typedef int32_t  q31_t;
typedef uint32_t q32_t;
typedef int64_t  q63_t;
typedef uint8_t  uq7_t;
typedef uint16_t uq15_t;
#endif

typedef enum {
    ARM_MATH_SUCCESS        = 0,
    ARM_MATH_ARGUMENT_ERROR = 1,
    ARM_MATH_LENGTH_ERROR   = 2,
    ARM_MATH_SIZE_MISMATCH  = 3,
    ARM_MATH_NANINF         = 4,
    ARM_MATH_SINGULAR       = 5,
    ARM_MATH_TEST_FAILURE   = 6
} arm_status;

/* ── Compiler attribute shims ──────────────────────────────────────────── */
#ifndef __STATIC_FORCEINLINE
#  define __STATIC_FORCEINLINE static inline __attribute__((always_inline))
#endif
#ifndef __STATIC_INLINE
#  define __STATIC_INLINE static inline
#endif
#ifndef __WEAK
#  define __WEAK __attribute__((weak))
#endif

/* ── ARM DSP intrinsics — portable C implementations ───────────────────── */

/* __SMLAD – Signed Multiply Accumulate Dual
 * acc += (int16_t)(a[15:0])  * (int16_t)(b[15:0])
 *      + (int16_t)(a[31:16]) * (int16_t)(b[31:16])  */
static inline int32_t __SMLAD(int32_t a, int32_t b, int32_t acc)
{
    return acc
         + (int32_t)((int16_t)(a & 0xFFFF)         * (int16_t)(b & 0xFFFF))
         + (int32_t)((int16_t)((a >> 16) & 0xFFFF)  * (int16_t)((b >> 16) & 0xFFFF));
}

/* __SMLADX – __SMLAD with b's halfwords swapped */
static inline int32_t __SMLADX(int32_t a, int32_t b, int32_t acc)
{
    return acc
         + (int32_t)((int16_t)(a & 0xFFFF)         * (int16_t)((b >> 16) & 0xFFFF))
         + (int32_t)((int16_t)((a >> 16) & 0xFFFF)  * (int16_t)(b & 0xFFFF));
}

/* __PKHBT – Pack Halfword Bottom-Top
 * result[15:0]  = a[15:0]
 * result[31:16] = (b << shift)[31:16]  */
static inline uint32_t __PKHBT(uint32_t a, uint32_t b, uint32_t shift)
{
    return ((uint32_t)(a) & 0x0000FFFFU)
         | (((uint32_t)(b) << shift) & 0xFFFF0000U);
}

/* __PKHTB – Pack Halfword Top-Bottom
 * result[31:16] = a[31:16]
 * result[15:0]  = (b >> shift)[15:0]  */
static inline uint32_t __PKHTB(uint32_t a, uint32_t b, uint32_t shift)
{
    return ((uint32_t)(a) & 0xFFFF0000U)
         | (((uint32_t)(b) >> shift) & 0x0000FFFFU);
}

/* __SADD16 – Signed parallel Add two 16-bit pairs */
static inline uint32_t __SADD16(uint32_t a, uint32_t b)
{
    int32_t lo = (int32_t)(int16_t)(a & 0xFFFF)         + (int32_t)(int16_t)(b & 0xFFFF);
    int32_t hi = (int32_t)(int16_t)((a >> 16) & 0xFFFF)  + (int32_t)(int16_t)((b >> 16) & 0xFFFF);
    return (uint32_t)(lo & 0xFFFF) | ((uint32_t)(hi & 0xFFFF) << 16);
}

/* __SXTB16 – Sign-extend bytes 0 and 2 to 16-bit halfwords */
static inline uint32_t __SXTB16(uint32_t a)
{
    return (uint32_t)(
        ((uint32_t)(uint16_t)(int16_t)(int8_t)( a        & 0xFF))
      | ((uint32_t)(uint16_t)(int16_t)(int8_t)((a >> 16) & 0xFF) << 16)
    );
}

/* __UXTB16 – Zero-extend bytes 0 and 2 to 16-bit halfwords */
static inline uint32_t __UXTB16(uint32_t a)
{
    return (uint32_t)(
        ((uint32_t)(a & 0x000000FFU))
      | ((uint32_t)(a & 0x00FF0000U))
    );
}

/* __ROR – Rotate Right */
static inline uint32_t __ROR(uint32_t op, uint32_t n)
{
    n &= 31U;
    return n ? (op >> n) | (op << (32U - n)) : op;
}

/* __SSAT – Signed Saturate to -(2^(sat-1)) … 2^(sat-1)-1 */
static inline int32_t __SSAT(int32_t val, uint32_t sat)
{
    if (sat == 0U || sat > 31U) return val;
    int32_t maxv =  (int32_t)((1U << (sat - 1U)) - 1U);
    int32_t minv = -(int32_t)(1U << (sat - 1U));
    return val > maxv ? maxv : val < minv ? minv : val;
}

/* __USAT – Unsigned Saturate to 0 … 2^sat-1 */
static inline uint32_t __USAT(int32_t val, uint32_t sat)
{
    if (sat >= 32U) return (val < 0) ? 0U : (uint32_t)val;
    uint32_t maxv = (1U << sat) - 1U;
    if (val < 0)              return 0U;
    if ((uint32_t)val > maxv) return maxv;
    return (uint32_t)val;
}

/* ── CMSIS-NN memory helpers (used by img2col_element.h) ───────────────── */

/* arm_nn_read_q7x4_ia – read 4 signed bytes as a packed q31, advance ptr */
static inline q31_t arm_nn_read_q7x4_ia(const q7_t **src)
{
    q31_t val;
    memcpy(&val, *src, 4);
    *src += 4;
    return val;
}

/* b2_nn_read_q7x4_ia – TinyEngine variant (2-bit packed), same layout */
static inline q31_t b2_nn_read_q7x4_ia(const q7_t **src)
{
    q31_t val;
    memcpy(&val, *src, 4);
    *src += 4;
    return val;
}

/* b4_nn_read_q7x4_ia – TinyEngine variant (4-bit packed), same layout */
static inline q31_t b4_nn_read_q7x4_ia(const q7_t **src)
{
    q31_t val;
    memcpy(&val, *src, 4);
    *src += 4;
    return val;
}

/* write_q15x2_ia – write two q15 values packed as q31, advance ptr */
static inline void write_q15x2_ia(q15_t **dst, q31_t val)
{
    memcpy(*dst, &val, 4);
    *dst += 2;
}
