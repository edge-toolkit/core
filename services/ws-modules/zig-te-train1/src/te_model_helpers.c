/*
 * Model-agnostic helpers. The buffer0-touching ones (te_get_pooled_*,
 * te_train_step) and lr/blr setters live INSIDE each codegen-MODEL/source/
 * genModel.c because `static signed char buffer[]` and `static float lr/blr`
 * in the headers are file-local — only the TU that ALSO compiles
 * invoke_inf() can read or mutate them. Helpers that don't touch those
 * live here, shared across all three model builds.
 */
#include <stdint.h>

#ifndef POOLED_COUNT
#  error "POOLED_COUNT must be defined in build.zig per model"
#endif
#ifndef PEAK_MEM_VALUE
#  error "PEAK_MEM_VALUE must be defined in build.zig per model"
#endif
#ifndef MODEL_SIZE_VALUE
#  error "MODEL_SIZE_VALUE must be defined in build.zig per model"
#endif

int te_pooled_feature_count(void) { return POOLED_COUNT; }

void te_get_static_memory(int32_t *out) {
    if (!out) return;
    out[0] = PEAK_MEM_VALUE;
    out[1] = MODEL_SIZE_VALUE;
}

const char *te_backend_name(void) {
#ifdef BACKEND_NAME
    return BACKEND_NAME;
#else
    return "unknown";
#endif
}
