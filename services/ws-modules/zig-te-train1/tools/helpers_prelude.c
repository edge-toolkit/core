
/* ── Extern prototypes prepended near the top of genModel.c by codegen.sh ──
 * These are inserted before any call site so the C compiler doesn't create
 * an implicit declaration with mismatched argument promotion. Without these,
 * the float args at the call sites would get default-promoted (e.g. `float`
 * → `double`), producing a wasm signature mismatch against the real (float)-
 * typed implementations in src/te_int8input_shim.c.
 *
 * These specific kernels exist in mbv2 and proxyless codegen output (sparse
 * update on a `_int8input` pointwise group conv); mcunet's codegen doesn't
 * happen to emit calls to these signatures, so the extra prototypes are a
 * no-op for that model.
 */
extern int group_conv_fp_kernel4_stride1_pad0_in4x4_out1x1_uniweight_4row16col_int8input_inplace(
    const float *input_data,
    unsigned short input_h, unsigned short input_w, unsigned short input_depth,
    const float *filter_data, const float *bias_data,
    signed char *output_weight_data,
    unsigned short output_h, unsigned short output_w, unsigned short output_depth,
    float output_act_min, float output_act_max,
    float *im2col_data, unsigned short batches, unsigned short groups,
    const float *scales, float learning_rate);
extern int group_conv_fp_kernel8_stride1_pad0_in8x8_out1x1_uniweight_4row16col_int8input_inplace(
    const float *input_data,
    unsigned short input_h, unsigned short input_w, unsigned short input_depth,
    const float *filter_data, const float *bias_data,
    signed char *output_weight_data,
    unsigned short output_h, unsigned short output_w, unsigned short output_depth,
    float output_act_min, float output_act_max,
    float *im2col_data, unsigned short batches, unsigned short groups,
    const float *scales, float learning_rate);
