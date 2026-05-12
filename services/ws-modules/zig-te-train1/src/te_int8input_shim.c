/*
 * Wrappers for the `_int8input_inplace` variants of group_conv_fp_kernel*
 * that the sparse_bp codegen calls for mbv2 + proxyless but that aren't
 * shipped in vendor/tinyengine/kernels/fp_backward_op/.
 *
 * mcunet's sparse_bp-49kb config trains layers where the upstream gradient
 * is already fp32 by the time it reaches the weight-update kernel — so the
 * codegen calls `..._inplace` (no _int8input suffix) and the existing
 * vendor kernel works. mbv2-123kb and proxyless-74kb hit weight-update
 * sites where the upstream is still int8 (a layer earlier in the gradient
 * chain) so the codegen emits `..._int8input_inplace`, but that variant
 * was never written.
 *
 * Cheapest correct implementation: convert the int8 input to fp32 in a
 * stack scratch buffer, then delegate to the existing fp32 kernel. The
 * scratch size is bounded by 8*8*max_channels = 64*max(input_depth) which
 * peaks around 64 * 56 = 3584 floats = 14 KB for mbv2's worst weight
 * (v17_conv_0_weight at input 4x4x56), well within the wasm stack budget.
 */
#include <stdint.h>

typedef int tinyengine_status_fp;
#define STATE_SUCCESS_fp 0

/* fp32 originals shipped with tinyengine. */
extern tinyengine_status_fp group_conv_fp_kernel4_stride1_pad0_in4x4_out1x1_uniweight_4row16col_inplace(
    const float *input_data,
    uint16_t input_h, uint16_t input_w, uint16_t input_depth,
    const float *filter_data, const float *bias_data,
    int8_t *output_weight_data,
    uint16_t output_h, uint16_t output_w, uint16_t output_depth,
    float output_act_min, float output_act_max,
    float *im2col_data, uint16_t batches, uint16_t groups,
    const float *scales, float learning_rate);

extern tinyengine_status_fp group_conv_fp_kernel8_stride1_pad0_in8x8_out1x1_uniweight_4row16col_inplace(
    const float *input_data,
    uint16_t input_h, uint16_t input_w, uint16_t input_depth,
    const float *filter_data, const float *bias_data,
    int8_t *output_weight_data,
    uint16_t output_h, uint16_t output_w, uint16_t output_depth,
    float output_act_min, float output_act_max,
    float *im2col_data, uint16_t batches, uint16_t groups,
    const float *scales, float learning_rate);

/* The codegen passes the int8 buffer cast to float*. Cast back, then expand
 * to a fp32 scratch and call the fp32 kernel. We need enough stack room
 * for input_h * input_w * input_depth floats. */
#define SHIM_MAX_SCRATCH 4096   /* floats; covers up to 4*4*256 or 8*8*64 */

#define DEFINE_SHIM(KERN_NAME, KERN_BASE)                                                                            \
tinyengine_status_fp KERN_NAME(                                                                                      \
    const float *input_data,                                                                                         \
    uint16_t input_h, uint16_t input_w, uint16_t input_depth,                                                        \
    const float *filter_data, const float *bias_data,                                                                \
    int8_t *output_weight_data,                                                                                      \
    uint16_t output_h, uint16_t output_w, uint16_t output_depth,                                                     \
    float output_act_min, float output_act_max,                                                                      \
    float *im2col_data, uint16_t batches, uint16_t groups,                                                           \
    const float *scales, float learning_rate)                                                                        \
{                                                                                                                    \
    /* Reinterpret the buffer as the underlying int8 storage. */                                                     \
    const int8_t *in_i8 = (const int8_t *)input_data;                                                                 \
    const int n = (int)input_h * (int)input_w * (int)input_depth;                                                    \
    static float s_scratch[SHIM_MAX_SCRATCH];                                                                        \
    if (n > SHIM_MAX_SCRATCH) {                                                                                      \
        /* Unexpectedly large input for this kernel; codegen-config bug — */                                         \
        /* return failure rather than scribble past the buffer. */                                                   \
        return -1;                                                                                                   \
    }                                                                                                                \
    for (int i = 0; i < n; i++) s_scratch[i] = (float)in_i8[i];                                                      \
    return KERN_BASE(                                                                                                \
        s_scratch, input_h, input_w, input_depth,                                                                    \
        filter_data, bias_data,                                                                                      \
        output_weight_data, output_h, output_w, output_depth,                                                        \
        output_act_min, output_act_max,                                                                              \
        im2col_data, batches, groups,                                                                                \
        scales, learning_rate);                                                                                      \
}

DEFINE_SHIM(group_conv_fp_kernel4_stride1_pad0_in4x4_out1x1_uniweight_4row16col_int8input_inplace,
            group_conv_fp_kernel4_stride1_pad0_in4x4_out1x1_uniweight_4row16col_inplace)

DEFINE_SHIM(group_conv_fp_kernel8_stride1_pad0_in8x8_out1x1_uniweight_4row16col_int8input_inplace,
            group_conv_fp_kernel8_stride1_pad0_in8x8_out1x1_uniweight_4row16col_inplace)
