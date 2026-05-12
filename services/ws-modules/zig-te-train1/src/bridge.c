/*
 * C bridge for the Zig training agent — restored generated-score readout
 * plus a stable binary prototype head for the demo's person/scene task.
 *
 * Background: we regenerated the 49KB sparse-bp triplet (assets/
 * 49kb-int8-graph.json + .pkl + scale.json) through tiny-training's
 * compilation pipeline using a pure-Python mcu_ops_shim.py on stock
 * apache-tvm. The new codegen restores the on-graph training surface:
 *   - 128x128x3 int8 input at &buffer0[65536]
 *   - 10-class head (mcunet-5fps backbone — same as the original
 *     reference triplet, hence same feature-collapse risk)
 *   - Pooled 160-dim features at &buffer0[34592] after invoke_inf()
 *   - invoke(labels) runs forward + backward and updates the 25
 *     v9..v15_* sparse-update tensors via QAS gradient scaling
 *
 * Path C training: te_train_step(label) builds a one-hot label vector
 * and invokes the codegen. The default demo path uses te_train_binary_step()
 * because the generated graph still backprops through a 10-way loss while
 * the UI task is binary.
 *
 * Surface:
 *   te_init()                       zero binary head, mark ready
 *   te_input_width/height/channels  graph dimension queries (80/80/3)
 *   te_num_classes                  codegen head class count
 *   te_input_ptr()                  pointer to the int8 input buffer
 *   te_input_size()                 80*80*3 = 19200 bytes
 *   te_run_inference()              invoke_inf() (forward only)
 *   te_get_logits(out)              copy 2 int8 head outputs
 *   te_get_scores(out)              fp32-resolution scores × 1024 → int32
 *   te_get_input_sig(out, n)        first n bytes of getInput()
 *   te_get_pooled_sig(out, n)       first n of the 160 int8 pooled features
 *   te_get_binary_scores(out)       fp32 binary-head logits × 1024 → int32
 *   te_train_binary_step(label)     update binary prototype head
 *   te_set_binary_lr / te_get_binary_lr
 *   te_get_binary_debug(out)        diagnostic counters for the demo
 *   te_reset_weights()              restore sparse-update tensors and
 *                                   re-zero the binary head
 */
#include "arm_compat.h"
#include <stdint.h>
#include <math.h>
#include <string.h>
#include "genNN.h"

#ifndef NUM_CLASSES
#  define NUM_CLASSES 2
#endif
#ifndef INPUT_W
#  define INPUT_W 80
#endif
#ifndef INPUT_H
#  define INPUT_H 80
#endif
#ifndef INPUT_C
#  define INPUT_C 3
#endif

static int s_ready = 0;

/* The fp32 binary prototype wrapper. Kept at 2 classes regardless of NUM_CLASSES
 * because: (1) the demo task is binary (person vs scene), (2) widening to
 * 10-way softmax dilutes the gradient — the 8 untrained classes contribute
 * exp(0)=1 each to the denominator, scaling the effective learning rate on
 * the 2 active classes down by ~5x. The te_get_binary_scores accessor
 * zero-pads the remaining NUM_CLASSES-2 slots so consumers expecting an
 * array of NUM_CLASSES ints see valid values everywhere. */
#define BINARY_CLASSES 2
/* Max across all 3 supported backbones — mcunet 160, mbv2 112, proxyless 96.
 * Sized generously so a future backbone with a wider final pool fits without
 * rebuilding. The actual count at runtime comes from te_pooled_feature_count(). */
#define BINARY_FEATURES_MAX 256
#define BINARY_SCORE_SCALE 1024.0f

/* Provided by codegen-MODEL/source/te_snapshot.c. The mcunet build links the
 * real snapshot/reset/diff implementation; mbv2/proxyless link no-op stubs
 * because their fwd-only codegen has no trainable v* tensors. */
extern void te_snapshot_weights(void);
extern void te_reset_weights_to_initial(void);
extern int  te_snapshot_ready(void);
extern void te_get_train_debug(int32_t *out);

/* Provided by src/te_model_helpers.c with per-model build defines.
 * te_train_step is invoke(labels) for mcunet, invoke_inf() for fwd-only. */
extern void te_train_step(int label);
extern void te_get_pooled_features(int32_t *out);
extern void te_get_pooled_sig(signed char *out, int n);
extern void te_get_static_memory(int32_t *out);
extern int  te_pooled_feature_count(void);

/* Stable nearest-centroid readout over pooled features. The previous online
 * SGD head could drift into a one-class solution after enough epochs. Running
 * class means are order-independent and do not decay back toward 50%. */
#define PROTOTYPE_LOGIT_GAIN 512.0f
static float s_binary_sum[BINARY_CLASSES][BINARY_FEATURES_MAX];
static int s_binary_count[BINARY_CLASSES];
static int s_n_features = 0;  /* set in te_init from te_pooled_feature_count() */
static float s_binary_lr = 0.005f;
static int s_binary_updates = 0;

static void te_reset_binary_head(void) {
    memset(s_binary_sum, 0, sizeof(s_binary_sum));
    memset(s_binary_count, 0, sizeof(s_binary_count));
    s_binary_updates = 0;
}

static void te_binary_logits(float *out) {
    for (int cls = 0; cls < BINARY_CLASSES; cls++) out[cls] = 0.0f;
    for (int cls = 0; cls < BINARY_CLASSES; cls++) {
        if (s_binary_count[cls] <= 0) return;
    }

    int32_t features[BINARY_FEATURES_MAX];
    te_get_pooled_features(features);
    const int n = s_n_features;
    for (int cls = 0; cls < BINARY_CLASSES; cls++) {
        float score = 0.0f;
        const float inv_count = 1.0f / (float)s_binary_count[cls];
        for (int ch = 0; ch < n; ch++) {
            const float x = (float)features[ch] * (1.0f / 128.0f);
            const float mean = s_binary_sum[cls][ch] * inv_count;
            score += x * mean - 0.5f * mean * mean;
        }
        out[cls] = score * PROTOTYPE_LOGIT_GAIN;
    }
}

void te_get_binary_scores(int32_t *out) {
    if (!out) return;
    for (int i = 0; i < NUM_CLASSES; i++) {
        out[i] = 0;
    }
    float logits[BINARY_CLASSES];
    te_binary_logits(logits);
    for (int i = 0; i < BINARY_CLASSES; i++) {
        out[i] = (int32_t)(logits[i] * BINARY_SCORE_SCALE);
    }
}

/* One prototype update. invoke_inf must run first to populate the pooled
 * features at the model-specific offset (mcunet buffer0[34592], mbv2/proxyless
 * buffer0[0]). */
void te_train_binary_step(int label) {
    invoke_inf();
    if (label < 0 || label >= BINARY_CLASSES) return;

    int32_t features[BINARY_FEATURES_MAX];
    te_get_pooled_features(features);
    const int n = s_n_features;
    for (int ch = 0; ch < n; ch++) {
        s_binary_sum[label][ch] += (float)features[ch] * (1.0f / 128.0f);
    }
    s_binary_count[label]++;
    s_binary_updates++;
}

void te_set_binary_lr(float v) {
    if (v > 0.0f && v < 10.0f) s_binary_lr = v;
}

float te_get_binary_lr(void) {
    return s_binary_lr;
}

/* Diagnostic counters for the demo's training-state verifier. Layout:
 *   0 weights_nonzero    1 weights_abs_sum_e6   2 weights_hash
 *   3 updates            4 lr × 1e6
 */
void te_get_binary_debug(int32_t *out) {
    if (!out) return;
    int32_t changed = 0;
    int64_t abs_sum = 0;
    uint32_t hash = 2166136261u;
    const unsigned char *bytes = (const unsigned char *)s_binary_sum;
    for (int i = 0; i < (int)sizeof(s_binary_sum); i++) {
        hash = (hash ^ bytes[i]) * 16777619u;
    }
    const int n = s_n_features;
    for (int cls = 0; cls < BINARY_CLASSES; cls++) {
        for (int ch = 0; ch < n; ch++) {
            const float v = s_binary_sum[cls][ch];
            if (v != 0.0f) changed++;
            const float a = v < 0.0f ? -v : v;
            const int32_t scaled = (int32_t)(a * 1000000.0f);
            if (abs_sum < 2000000000 - scaled) abs_sum += scaled;
        }
    }
    out[0] = changed;
    out[1] = (int32_t)abs_sum;
    out[2] = (int32_t)hash;
    out[3] = s_binary_updates;
    out[4] = (int32_t)(s_binary_lr * 1000000.0f);
}

/* ── Init / reset ─────────────────────────────────────────────────────────
 * Snapshot the codegen's trainable sparse-update tensors once, then reset
 * restores both those tensors and the fp32 binary head layered on top. */
int te_init(void) {
    if (s_ready) return 0;
    /* Capture the runtime pooled-feature width before any binary-head work
     * uses it. Clamp to the buffer max in case a future codegen widens it. */
    s_n_features = te_pooled_feature_count();
    if (s_n_features < 0) s_n_features = 0;
    if (s_n_features > BINARY_FEATURES_MAX) s_n_features = BINARY_FEATURES_MAX;
    te_reset_binary_head();
    /* Snapshot the pretrained trainable tensors before any training step
     * runs. te_reset_weights() then restores them; without this, "reset"
     * would leave whatever the last training step produced in place. For
     * fwd-only models this is a no-op (see codegen-MODEL/source/te_snapshot.c). */
    te_snapshot_weights();
    s_ready = 1;
    return 0;
}

void te_reset_weights(void) {
    te_reset_binary_head();
    te_reset_weights_to_initial();
}

int te_is_ready(void)        { return s_ready; }
int te_input_width(void)     { return INPUT_W; }
int te_input_height(void)    { return INPUT_H; }
int te_input_channels(void)  { return INPUT_C; }
int te_num_classes(void)     { return NUM_CLASSES; }
int te_input_size(void)      { return INPUT_W * INPUT_H * INPUT_C; }

signed char *te_input_ptr(void) {
    return getInput();
}

void te_run_inference(void) {
    invoke_inf();
}

void te_get_logits(signed char *out) {
    if (!out) return;
    signed char *raw = getOutput();
    for (int i = 0; i < NUM_CLASSES; i++) out[i] = raw[i];
}

/* Promote the codegen's int8 head logits to int32 (× 1024) so the JSON reply
 * can carry them without floats. The Path C int8 head tends to saturate to 0
 * across all classes pre-training (head zero-point quantization issue, see
 * project_webapp_training_bugs); the demo's scoreAt sums these with the fp32
 * binary-head's binary_scores so a saturated `scores` doesn't poison the
 * verdict. */
void te_get_scores(int32_t *out) {
    if (!out) return;
    signed char *raw = getOutput();
    for (int i = 0; i < NUM_CLASSES; i++) out[i] = (int32_t)raw[i] * 1024;
}

/* Read the first n bytes of getInput() (the int8 NHWC input buffer at
 * &buffer0[25600]) into the caller's buffer. Demo verifier reads this
 * after each js_get_file_bin to confirm the bytes JS sent actually
 * landed in wasm memory. */
void te_get_input_sig(signed char *out, int n) {
    if (!out || n <= 0) return;
    signed char *src = getInput();
    const int total = INPUT_W * INPUT_H * INPUT_C;
    if (n > total) n = total;
    for (int i = 0; i < n; i++) out[i] = src[i];
}

/* Compose the codegen's static memory budget with this TU's runtime
 * overhead (the fp32 binary-head wrapper that doesn't exist in the
 * codegen). Matches the paper's accounting: peak SRAM + model flash +
 * trainable-state memory (Section 4 / Figure 10 of arXiv:2206.15472).
 *
 * Layout (all values in bytes):
 *   out[0] = peak_sram         buffer0 activation budget (PEAK_MEM)
 *   out[1] = model_flash       static const data (MODEL_SIZE)
 *   out[2] = binary_head       fp32 prototype state we layer on top
 *   out[3] = train_sram_peak   peak SRAM during a training step:
 *                              activations + binary head + small stack
 *   out[4] = input_bytes       INPUT_W * INPUT_H * INPUT_C (one frame)
 *   out[5] = ft_full_sram      paper "FT-Full" variant — full backward
 *                              pass, every weight trainable, no codegen
 *                              optimizations. Static analysis from the
 *                              full_bp triplet through tinyengine's
 *                              GeneralMemoryScheduler.
 *   out[6] = ft_su_sram        paper "FT-SU" — sparse update with NO
 *                              in-place / reorder optimization. Same
 *                              sparse_bp triplet but inplace=False and
 *                              sort_by_lifetime=False.
 *   out[7] = ft_sur_sram       paper "FT-SU+R" — sparse update + the
 *                              in-place + lifetime-sort reorder
 *                              optimizations. Equals out[0] / peak_sram
 *                              because this wasm is built with all
 *                              optimizations on. Repeated explicitly so
 *                              the JSON shape stays uniform.
 *
 * The numbers ignore tiny scalar bookkeeping (s_binary_lr, counters,
 * etc.) and stack frames of the kernel calls — both are small and
 * irrelevant to the headline figure. */
void te_get_memory(int32_t *out) {
    if (!out) return;
    int32_t base[2] = { 0, 0 };
    te_get_static_memory(base);
    const int32_t binary_head_bytes =
        (int32_t)(sizeof(s_binary_sum) + sizeof(s_binary_count));
    out[0] = base[0];
    out[1] = base[1];
    out[2] = binary_head_bytes;
    out[3] = base[0] + binary_head_bytes;
    out[4] = (int32_t)(INPUT_W * INPUT_H * INPUT_C);
    out[5] = (int32_t)FT_FULL_SRAM;
    out[6] = (int32_t)FT_SU_SRAM;
    out[7] = (int32_t)FT_SUR_SRAM;
}
