/* WASM build stub — shadows CMSIS-NN arm_nnsupportfunctions.h.
 * Provides the subset of CMSIS-NN support functions used by TinyEngine
 * kernels, implemented as portable C (no ARM DSP intrinsics required). */
#pragma once
#include "arm_compat.h"

/* ── Basic min/max macros (mirroring the real arm_nnsupportfunctions.h) ─── */
#ifndef MAX
#  define MAX(A, B) ((A) > (B) ? (A) : (B))
#endif
#ifndef MIN
#  define MIN(A, B) ((A) < (B) ? (A) : (B))
#endif
#ifndef CLAMP
#  define CLAMP(x, h, l) MAX(MIN((x), (h)), (l))
#endif

/* ── q15x2 read with pointer advance ───────────────────────────────────── */
static inline q31_t arm_nn_read_q15x2_ia(const q15_t **in_q15)
{
    q31_t val;
    memcpy(&val, *in_q15, 4);
    *in_q15 += 2;
    return val;
}

/* ── q15x2 read (no advance) ────────────────────────────────────────────── */
static inline q31_t arm_nn_read_q15x2(const q15_t *in_q15)
{
    q31_t val;
    memcpy(&val, in_q15, 4);
    return val;
}

/* ── q7x4 write with pointer advance ───────────────────────────────────── */
static inline void arm_nn_write_q7x4_ia(q7_t **in, q31_t value)
{
    memcpy(*in, &value, 4);
    *in += 4;
}

/* ── __SXTB16_RORn – sign-extend bytes after rotate (used by read_and_pad) */
static inline uint32_t __SXTB16_RORn(uint32_t a, uint32_t n)
{
    return __SXTB16(__ROR(a, n));
}

/* ── read_and_pad – expand 4 q7 bytes into two packed q15 words ─────────── */
static inline const q7_t *read_and_pad(const q7_t *source, q31_t *out1, q31_t *out2)
{
    q31_t inA = arm_nn_read_q7x4_ia(&source);
    q31_t inAbuf1 = __SXTB16_RORn((uint32_t)inA, 8);
    q31_t inAbuf2 = __SXTB16(inA);
    *out2 = (int32_t)__PKHTB(inAbuf1, inAbuf2, 16);
    *out1 = (int32_t)__PKHBT(inAbuf2, inAbuf1, 16);
    return source;
}

/* ── read_and_pad_reordered – same but with swapped halfword order ──────── */
static inline const q7_t *read_and_pad_reordered(const q7_t *source, q31_t *out1, q31_t *out2)
{
    q31_t inA = arm_nn_read_q7x4_ia(&source);
    *out2 = __SXTB16(__ROR((uint32_t)inA, 8));
    *out1 = __SXTB16(inA);
    return source;
}
