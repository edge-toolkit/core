
/* ── Per-model te_* helpers (appended to genModel.c by tools/codegen.sh) ──
 * These live in the same TU as invoke_inf() because the codegen marks the
 * activation buffer + lr/blr as `static` in headers — they are file-local
 * per TU. Any other source file gets its OWN private copy of buffer0/lr/blr
 * and writes there have no effect on inference.
 *
 * POOLED_OFFSET / POOLED_COUNT / HAS_TRAIN_GRAPH are -D'd in build.zig per
 * model (see ModelCfg / buildOne in that file).
 *
 * The extern prototypes for the int8input kernels live in helpers_prelude.c
 * and are inserted near the top of genModel.c — they have to come BEFORE
 * any call site to avoid C's implicit-declaration conflict.
 */

void te_set_lr(float v)  { lr  = v; }
void te_set_blr(float v) { blr = v; }
float te_get_lr(void)    { return lr; }
float te_get_blr(void)   { return blr; }

void te_get_pooled_sig(signed char *out, int n) {
    if (!out || n <= 0) return;
    const signed char *src = (const signed char *)&buffer0[POOLED_OFFSET];
    int cap = POOLED_COUNT;
    if (n > cap) n = cap;
    for (int i = 0; i < n; i++) out[i] = src[i];
}

void te_get_pooled_features(int32_t *out) {
    if (!out) return;
    const int8_t *src = (const int8_t *)&buffer0[POOLED_OFFSET];
    for (int ch = 0; ch < POOLED_COUNT; ch++) out[ch] = (int32_t)src[ch];
}

/* Runtime working-memory probe. The activation arena is `buffer[]` (aliased by
 * buffer0) whose size is PEAK_MEM_VALUE (-D'd in build.zig, == the model's
 * static peak SRAM). Fill it with a canary byte before a run, then count the
 * bytes that changed afterward — that's the actual touched working set, which
 * main.zig reports as `arena_touched`. Both must live in this TU because
 * buffer0 is file-local `static`. */
void te_arena_fill_canary(signed char canary) {
    for (int i = 0; i < PEAK_MEM_VALUE; i++) buffer0[i] = canary;
}

int te_arena_count_touched(signed char canary) {
    int n = 0;
    for (int i = 0; i < PEAK_MEM_VALUE; i++) if (buffer0[i] != canary) n++;
    return n;
}

#if HAS_TRAIN_GRAPH
static float s_te_labels[10];
void te_train_step(int label) {
    invoke_inf();
    if (label < 0 || label >= 10) return;
    for (int i = 0; i < 10; i++) s_te_labels[i] = (i == label) ? 1.0f : 0.0f;
    invoke(s_te_labels);
    for (int i = 0; i < 10; i++) s_te_labels[i] = 0.0f;
}
#else
void te_train_step(int label) {
    (void)label;
    invoke_inf();
}
#endif
