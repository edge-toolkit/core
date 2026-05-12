/*
 * Compatibility kernels for mcunet sparse-backward codegen variants that are
 * emitted by the patched TinyEngine generator but are not shipped in the
 * vendored kernel source set. These are generic wasm-friendly fallbacks.
 */
#include <stdint.h>
#include <string.h>

typedef int tinyengine_status_fp;
#define STATE_SUCCESS_fp 0

static float clampf_te(float v, float lo, float hi) {
    return v < lo ? lo : (v > hi ? hi : v);
}

static int bit_is_set(const unsigned char *mask, int i) {
    return (mask[i >> 3] & (1u << (i & 7))) != 0;
}

void sum_4D_exclude_int8(const signed char *input, int d1, int d2, int d3, int d4, int axis, int *output) {
    if (axis == 0) {
        for (int i = 0; i < d1; i++) {
            int sum = 0;
            for (int j = 0; j < d2; j++) for (int m = 0; m < d3; m++) for (int n = 0; n < d4; n++)
                sum += input[((i * d2 + j) * d3 + m) * d4 + n];
            output[i] = sum;
        }
    } else if (axis == 1) {
        for (int j = 0; j < d2; j++) {
            int sum = 0;
            for (int i = 0; i < d1; i++) for (int m = 0; m < d3; m++) for (int n = 0; n < d4; n++)
                sum += input[((i * d2 + j) * d3 + m) * d4 + n];
            output[j] = sum;
        }
    } else if (axis == 2) {
        for (int m = 0; m < d3; m++) {
            int sum = 0;
            for (int i = 0; i < d1; i++) for (int j = 0; j < d2; j++) for (int n = 0; n < d4; n++)
                sum += input[((i * d2 + j) * d3 + m) * d4 + n];
            output[m] = sum;
        }
    } else if (axis == 3) {
        for (int n = 0; n < d4; n++) {
            int sum = 0;
            for (int i = 0; i < d1; i++) for (int j = 0; j < d2; j++) for (int m = 0; m < d3; m++)
                sum += input[((i * d2 + j) * d3 + m) * d4 + n];
            output[n] = sum;
        }
    }
}

void where_zeros_int8_inplace_bit(const unsigned char *mask, int size, signed char *inout) {
    for (int i = 0; i < size; i++) {
        if (!bit_is_set(mask, i)) inout[i] = 0;
    }
}

void strided_slice_4Dto4D_int8(const signed char *input,
    int d1, int d2, int d3, int d4,
    const unsigned short *begin, const unsigned short *end, const unsigned short *strides,
    signed char *output, int o1, int o2, int o3, int o4)
{
    (void)d1; (void)d2; (void)d3; (void)d4; (void)end;
    const int s0 = strides[0];
    const int s1 = strides[1] ? strides[1] : s0;
    const int s2 = strides[2] ? strides[2] : s0;
    const int s3 = strides[3] ? strides[3] : s0;
    for (int n = 0; n < o1; n++) for (int c = 0; c < o2; c++) for (int h = 0; h < o3; h++) for (int w = 0; w < o4; w++) {
        const int in_n = begin[0] + n * s0;
        const int in_c = begin[1] + c * s1;
        const int in_h = begin[2] + h * s2;
        const int in_w = begin[3] + w * s3;
        output[((n * o2 + c) * o3 + h) * o4 + w] = input[((in_n * d2 + in_c) * d3 + in_h) * d4 + in_w];
    }
}

static tinyengine_status_fp pointwise_fp_i8w(const float *input,
    unsigned short input_h, unsigned short input_w, unsigned short input_depth,
    const signed char *filter, const signed char *filter_flash, unsigned short partial_ch,
    float *output, unsigned short output_h, unsigned short output_w, unsigned short output_depth,
    float act_min, float act_max, int input_is_int8)
{
    (void)input_h; (void)input_w;
    const int pixels = (int)output_h * (int)output_w;
    const signed char *input_i8 = (const signed char *)input;
    for (int p = 0; p < pixels; p++) {
        for (int oc = 0; oc < output_depth; oc++) {
            float sum = 0.0f;
            for (int ic = 0; ic < input_depth; ic++) {
                const float x = input_is_int8 ? (float)input_i8[p * input_depth + ic] : input[p * input_depth + ic];
                const signed char *wbase = filter;
                int wic = ic;
                if (filter_flash && ic >= partial_ch) {
                    wbase = filter_flash;
                    wic = ic - partial_ch;
                }
                sum += x * (float)wbase[wic * output_depth + oc];
            }
            output[p * output_depth + oc] = clampf_te(sum, act_min, act_max);
        }
    }
    return STATE_SUCCESS_fp;
}

tinyengine_status_fp pointwise_conv_fp_1row10col_10inputdepth_IOHW_int8output_int8weight(
    const float *input_data, unsigned short input_h, unsigned short input_w, unsigned short input_depth,
    const signed char *filter_data, const float *bias_data, float *output_data,
    unsigned short output_h, unsigned short output_w, unsigned short output_depth,
    float output_act_min, float output_act_max, float *im2col_data, void *bitmask_buffer,
    unsigned short batches)
{
    (void)bias_data; (void)im2col_data; (void)bitmask_buffer; (void)batches;
    return pointwise_fp_i8w(input_data, input_h, input_w, input_depth, filter_data, 0, 0,
        output_data, output_h, output_w, output_depth, output_act_min, output_act_max, 0);
}

tinyengine_status_fp pointwise_conv_fp_4row4col_IOHW_int8output_int8weight(
    const float *input_data, unsigned short input_h, unsigned short input_w, unsigned short input_depth,
    const signed char *filter_data, const float *bias_data, float *output_data,
    unsigned short output_h, unsigned short output_w, unsigned short output_depth,
    float output_act_min, float output_act_max, float *im2col_data, void *bitmask_buffer,
    unsigned short batches)
{
    return pointwise_conv_fp_1row10col_10inputdepth_IOHW_int8output_int8weight(
        input_data, input_h, input_w, input_depth, filter_data, bias_data, output_data,
        output_h, output_w, output_depth, output_act_min, output_act_max, im2col_data,
        bitmask_buffer, batches);
}

tinyengine_status_fp pointwise_conv_4row4col_IOHW_int8input_int8weight(
    const float *input_data, unsigned short input_h, unsigned short input_w, unsigned short input_depth,
    const signed char *filter_data, const float *bias_data, float *output_data,
    unsigned short output_h, unsigned short output_w, unsigned short output_depth,
    float output_act_min, float output_act_max, float *im2col_data, void *norm_buffer,
    unsigned short batches)
{
    (void)bias_data; (void)im2col_data; (void)norm_buffer; (void)batches;
    return pointwise_fp_i8w(input_data, input_h, input_w, input_depth, filter_data, 0, 0,
        output_data, output_h, output_w, output_depth, output_act_min, output_act_max, 1);
}

tinyengine_status_fp pointwise_conv_4row4col_IOHW_int8input_int8weight_partialCH_8innercol(
    const float *input_data, unsigned short input_h, unsigned short input_w, unsigned short input_depth,
    const signed char *trainable_weight, const signed char *frozen_weight, unsigned short partial_ch,
    const float *bias_data, float *output_data, unsigned short output_h, unsigned short output_w,
    unsigned short output_depth, float output_act_min, float output_act_max, float *im2col_data,
    void *bitmask_buffer, unsigned short batches)
{
    (void)bias_data; (void)im2col_data; (void)bitmask_buffer; (void)batches;
    return pointwise_fp_i8w(input_data, input_h, input_w, input_depth, trainable_weight, frozen_weight, partial_ch,
        output_data, output_h, output_w, output_depth, output_act_min, output_act_max, 1);
}

tinyengine_status_fp pointwise_conv_4row4col_IOHW_int8input_int8weight_partialCH_4innercol(
    const float *input_data, unsigned short input_h, unsigned short input_w, unsigned short input_depth,
    const signed char *trainable_weight, const signed char *frozen_weight, unsigned short partial_ch,
    const float *bias_data, float *output_data, unsigned short output_h, unsigned short output_w,
    unsigned short output_depth, float output_act_min, float output_act_max, float *im2col_data,
    void *bitmask_buffer, unsigned short batches)
{
    return pointwise_conv_4row4col_IOHW_int8input_int8weight_partialCH_8innercol(
        input_data, input_h, input_w, input_depth, trainable_weight, frozen_weight, partial_ch,
        bias_data, output_data, output_h, output_w, output_depth, output_act_min, output_act_max,
        im2col_data, bitmask_buffer, batches);
}

static tinyengine_status_fp transpose_depthwise_fp(const float *input,
    unsigned short input_h, unsigned short input_w, unsigned short input_depth,
    const float *filter, float *output,
    unsigned short output_h, unsigned short output_w, unsigned short output_depth,
    float act_min, float act_max, int kernel, int stride, int in_pad)
{
    (void)output_depth;
    memset(output, 0, (size_t)output_h * output_w * input_depth * sizeof(float));
    for (int ih = 0; ih < input_h; ih++) for (int iw = 0; iw < input_w; iw++) for (int c = 0; c < input_depth; c++) {
        const float x = input[(ih * input_w + iw) * input_depth + c];
        for (int kh = 0; kh < kernel; kh++) for (int kw = 0; kw < kernel; kw++) {
            const int oh = ih * stride + kh - in_pad;
            const int ow = iw * stride + kw - in_pad;
            if (oh < 0 || ow < 0 || oh >= output_h || ow >= output_w) continue;
            output[(oh * output_w + ow) * input_depth + c] += x * filter[(kh * kernel + kw) * input_depth + c];
        }
    }
    for (int i = 0; i < (int)output_h * output_w * input_depth; i++) output[i] = clampf_te(output[i], act_min, act_max);
    return STATE_SUCCESS_fp;
}

tinyengine_status_fp transpose_depthwise_conv_fp_kernel3_stride1_inpad1_outpad0_IOHW(
    float *input_output_data, unsigned short input_h, unsigned short input_w, unsigned short input_depth,
    const float *filter_data, const float *bias_data, float *output_data,
    unsigned short output_h, unsigned short output_w, unsigned short output_depth,
    float output_act_min, float output_act_max, float *im2col_data, void *norm_buffer,
    unsigned short batches, int pad_value)
{
    (void)bias_data; (void)im2col_data; (void)norm_buffer; (void)batches; (void)pad_value;
    return transpose_depthwise_fp(input_output_data, input_h, input_w, input_depth, filter_data, output_data,
        output_h, output_w, output_depth, output_act_min, output_act_max, 3, 1, 1);
}

tinyengine_status_fp transpose_depthwise_conv_fp_kernel5_stride1_inpad2_outpad0_IOHW(
    float *input_output_data, unsigned short input_h, unsigned short input_w, unsigned short input_depth,
    const float *filter_data, const float *bias_data, float *output_data,
    unsigned short output_h, unsigned short output_w, unsigned short output_depth,
    float output_act_min, float output_act_max, float *im2col_data, void *norm_buffer,
    unsigned short batches, int pad_value)
{
    (void)bias_data; (void)im2col_data; (void)norm_buffer; (void)batches; (void)pad_value;
    return transpose_depthwise_fp(input_output_data, input_h, input_w, input_depth, filter_data, output_data,
        output_h, output_w, output_depth, output_act_min, output_act_max, 5, 1, 2);
}

tinyengine_status_fp transpose_depthwise_conv_fp_kernel7_stride2_inpad3_outpad1_IOHW(
    float *input_data, unsigned short input_h, unsigned short input_w, unsigned short input_depth,
    const float *filter_data, const float *bias_data, float *output_data,
    unsigned short output_h, unsigned short output_w, unsigned short output_depth,
    float output_act_min, float output_act_max, float *im2col_data, void *norm_buffer,
    unsigned short batches, int pad_value)
{
    (void)bias_data; (void)im2col_data; (void)norm_buffer; (void)batches; (void)pad_value;
    return transpose_depthwise_fp(input_data, input_h, input_w, input_depth, filter_data, output_data,
        output_h, output_w, output_depth, output_act_min, output_act_max, 7, 2, 3);
}

static signed char quant_update(float v) {
    if (v > 127.0f) return 127;
    if (v < -128.0f) return -128;
    return (signed char)v;
}

tinyengine_status_fp accumulating_group_pointwise_conv_fp_in1x1_out1x1_1row10col_uniweight_inplace(
    const float *input_data, unsigned short input_x, unsigned short input_y, unsigned short input_ch,
    const float *filter_data, const float *bias_data, signed char *output_weight_data,
    unsigned short output_x, unsigned short output_y, unsigned short output_ch,
    float output_activation_min, float output_activation_max, float *im2col_data,
    unsigned short batches, unsigned short groups, const float *scales, float learning_rate)
{
    (void)input_x; (void)input_y; (void)bias_data; (void)output_x; (void)output_y;
    (void)output_activation_min; (void)output_activation_max; (void)im2col_data; (void)batches;
    const int per_group = output_ch / groups;
    for (int g = 0; g < groups; g++) {
        const float scale = scales ? scales[g] : 1.0f;
        for (int ic = 0; ic < input_ch; ic++) {
            const float x = input_data[ic];
            for (int oc = 0; oc < per_group; oc++) {
                const int idx = g * per_group + ic * per_group + oc;
                const float grad = x * filter_data[g * per_group + oc] * scale * learning_rate;
                output_weight_data[idx] = quant_update((float)output_weight_data[idx] - grad);
            }
        }
    }
    return STATE_SUCCESS_fp;
}

tinyengine_status_fp group_conv_kernel8_stride1_pad0_in8x8_out1x1_uniweight_4row16col_int8input_int8weight_inplace(
    const float *input_data, unsigned short input_h, unsigned short input_w, unsigned short input_depth,
    const float *filter_data, const float *bias_data, signed char *output_weight_data,
    unsigned short output_h, unsigned short output_w, unsigned short output_depth,
    float output_act_min, float output_act_max, float *im2col_data, void *norm_buffer,
    unsigned short batches, unsigned short groups, const float *scales, float learning_rate)
{
    (void)input_h; (void)input_w; (void)input_depth; (void)bias_data; (void)output_h; (void)output_w;
    (void)output_act_min; (void)output_act_max; (void)im2col_data; (void)norm_buffer; (void)batches;
    const signed char *input_i8 = (const signed char *)input_data;
    const int per_group = groups ? output_depth / groups : output_depth;
    for (int g = 0; g < groups; g++) {
        const float scale = scales ? scales[g] : 1.0f;
        for (int k = 0; k < per_group; k++) {
            const int idx = g * per_group + k;
            const float grad = (float)input_i8[g] * filter_data[idx] * scale * learning_rate;
            output_weight_data[idx] = quant_update((float)output_weight_data[idx] - grad);
        }
    }
    return STATE_SUCCESS_fp;
}

void permute4D_dim3012(const float *input, int d1, int d2, int d3, int d4, float *output) {
    for (int a = 0; a < d1; a++) for (int b = 0; b < d2; b++) for (int c = 0; c < d3; c++) for (int d = 0; d < d4; d++)
        output[((d * d1 + a) * d2 + b) * d3 + c] = input[((a * d2 + b) * d3 + c) * d4 + d];
}

void permute_groupconv_out(const float *input, int input_h, int input_w, int input_c, int out_per_group, int groups, float *output) {
    const int total = input_h * input_w * input_c * out_per_group * groups;
    for (int i = 0; i < total; i++) output[i] = input[i];
}

void te_reset_v15_accumulator(void) {}

void te_get_v15_accumulator_stats(int *out) {
    if (!out) return;
    out[0] = 0;
    out[1] = 0;
    out[2] = 0;
}
