// et_ws_zig_te_train1 — Zig agent that runs the TinyEngine MCUNetV3 VWW
// 10-class training pipeline as a wasm32-wasi module behind edge-toolkit's
// WebSocket + storage protocol.
//
// Mirrors tutorial/training/Src/main.cpp's model invocation pattern. TinyEngine
// defaults are LR=0.0008 / BLR=0.0004; build.zig overrides them lower for this
// browser demo to avoid immediate int8 head saturation.
// Host I/O — image fetch, label receipt, prediction reply — is routed through
// the same SharedArrayBuffer-bridged JS imports that zig-data1 uses, so this
// module slots into et-modules-service exactly like the Rust modules do.

const std = @import("std");
// ── JS imports (resolved by pkg/et_te_train1_worker.js) ────────────────────
extern fn js_log(ptr: [*]const u8, len: usize) void;
extern fn js_set_status(ptr: [*]const u8, len: usize) void;
extern fn js_ws_connect(url_ptr: [*]const u8, url_len: usize) void;
extern fn js_ws_send(ptr: [*]const u8, len: usize) void;
extern fn js_ws_disconnect() void;
extern fn js_ws_get_state(buf: [*]u8, max: usize) usize;
extern fn js_ws_get_agent_id(buf: [*]u8, max: usize) usize;
extern fn js_ws_pop_response(buf: [*]u8, max: usize) usize;
extern fn js_get_file_bin(url_ptr: [*]const u8, url_len: usize, buf: [*]u8, max: usize) usize;
extern fn js_put_file(url_ptr: [*]const u8, url_len: usize, body_ptr: [*]const u8, body_len: usize) void;
extern fn js_sleep_ms(ms: u32) void;
extern fn js_get_ws_url(buf: [*]u8, max: usize) usize;

// ── C bridge (src/bridge.c) ────────────────────────────────────────────────
extern fn te_init() c_int;
extern fn te_input_width() c_int;
extern fn te_input_height() c_int;
extern fn te_input_channels() c_int;
extern fn te_num_classes() c_int;
extern fn te_input_size() c_int;
extern fn te_input_ptr() [*]i8;
extern fn te_run_inference() void;
extern fn te_get_logits(out: [*]i8) void;
extern fn te_get_scores(out: [*]i32) void;
extern fn te_get_binary_scores(out: [*]i32) void;
extern fn te_train_step(label: c_int) void;
extern fn te_train_binary_step(label: c_int) void;
extern fn te_get_train_debug(out: [*]i32) void;
extern fn te_get_binary_debug(out: [*]i32) void;
extern fn te_get_input_sig(out: [*]i8, n: c_int) void;
extern fn te_get_pooled_sig(out: [*]i8, n: c_int) void;
extern fn te_get_memory(out: [*]i32) void;
extern fn te_arena_fill_canary(canary: i8) void;
extern fn te_arena_count_touched(canary: i8) c_int;
extern fn te_reset_weights() void;
extern fn te_set_binary_lr(v: f32) void;
extern fn te_get_binary_lr() f32;

// Note: previously we exported `exp` / `log` / `expf` / `logf` here delegating
// to std.math.exp / std.math.log so kernels (tte_exp_fp.c, log_softmax_fp.c)
// could link. That created infinite recursion: `std.math.exp` is `inline` and
// expands to `@exp(value)`, which on wasm32 has no native instruction and
// lowers to a call to the C symbol `exp` — i.e. back to our export. Each call
// pushed a frame until the wasm stack region exhausted and V8 reported
// "Maximum call stack size exceeded". With `link_libc = true` and the
// wasm32-wasi target, zig links musl's libm which already provides exp/log.

// ── Allocator (small — only used for log lines and JSON formatting) ────────
var heap: [128 * 1024]u8 = undefined;
var fba = std.heap.FixedBufferAllocator.init(&heap);
const alloc = fba.allocator();

fn logf_(comptime fmt: []const u8, args: anytype) void {
    const restore = fba.end_index;
    defer fba.end_index = restore;
    const msg = std.fmt.allocPrint(alloc, "[et-ws-zig-te-train1] " ++ fmt, args) catch return;
    js_log(msg.ptr, msg.len);
}

fn setStatus(comptime fmt: []const u8, args: anytype) void {
    const restore = fba.end_index;
    defer fba.end_index = restore;
    const msg = std.fmt.allocPrint(alloc, fmt, args) catch return;
    js_set_status(msg.ptr, msg.len);
}

fn sendJson(comptime fmt: []const u8, args: anytype) void {
    const restore = fba.end_index;
    defer fba.end_index = restore;
    const msg = std.fmt.allocPrint(alloc, fmt, args) catch return;
    js_ws_send(msg.ptr, msg.len);
}

// ── WebSocket / agent-protocol helpers ─────────────────────────────────────
fn waitState(want: []const u8) bool {
    var buf: [32]u8 = undefined;
    var i: u32 = 0;
    while (i < 200) : (i += 1) {
        const n = js_ws_get_state(&buf, buf.len);
        if (std.mem.eql(u8, buf[0..n], want)) return true;
        js_sleep_ms(50);
    }
    return false;
}

fn waitAgentId(buf: []u8) usize {
    var i: u32 = 0;
    while (i < 200) : (i += 1) {
        const n = js_ws_get_agent_id(buf.ptr, buf.len);
        if (n > 0) return n;
        js_sleep_ms(50);
    }
    return 0;
}

// Block until a WS message arrives, then return its length in buf.
fn popMessage(buf: []u8) usize {
    while (true) {
        const n = js_ws_pop_response(buf.ptr, buf.len);
        if (n > 0) return n;
        if (!waitState("connected")) return 0;
        js_sleep_ms(50);
    }
}

// ── Minimal JSON field reader (string and integer) ─────────────────────────
// Inputs are well-formed envelopes generated by the rest of edge-toolkit, so
// we don't need a full parser — just exact-key string and integer extraction.
fn findKey(json: []const u8, key: []const u8) ?usize {
    var i: usize = 0;
    while (i + key.len + 2 <= json.len) : (i += 1) {
        if (json[i] != '"') continue;
        if (i + 1 + key.len + 1 > json.len) return null;
        if (std.mem.eql(u8, json[i + 1 .. i + 1 + key.len], key) and json[i + 1 + key.len] == '"') {
            var j = i + 2 + key.len;
            while (j < json.len and (json[j] == ' ' or json[j] == ':' or json[j] == '\t')) : (j += 1) {}
            return j;
        }
    }
    return null;
}

fn readJsonString(json: []const u8, key: []const u8, out: []u8) ?[]u8 {
    const start = findKey(json, key) orelse return null;
    if (start >= json.len or json[start] != '"') return null;
    var j: usize = start + 1;
    var n: usize = 0;
    while (j < json.len and json[j] != '"' and n < out.len) : ({
        j += 1;
        n += 1;
    }) {
        if (json[j] == '\\' and j + 1 < json.len) {
            out[n] = json[j + 1];
            j += 1;
        } else {
            out[n] = json[j];
        }
    }
    return out[0..n];
}

fn readJsonFloat(json: []const u8, key: []const u8) ?f32 {
    const start = findKey(json, key) orelse return null;
    // Skip whitespace.
    var j = start;
    while (j < json.len and (json[j] == ' ' or json[j] == '\t')) : (j += 1) {}
    const num_start = j;
    if (j < json.len and (json[j] == '-' or json[j] == '+')) j += 1;
    while (j < json.len) : (j += 1) {
        const c = json[j];
        if (!((c >= '0' and c <= '9') or c == '.' or c == 'e' or c == 'E' or c == '-' or c == '+')) break;
    }
    if (j == num_start) return null;
    return std.fmt.parseFloat(f32, json[num_start..j]) catch null;
}

fn readJsonInt(json: []const u8, key: []const u8) ?i32 {
    const start = findKey(json, key) orelse return null;
    var j = start;
    var neg = false;
    if (j < json.len and json[j] == '-') {
        neg = true;
        j += 1;
    }
    var v: i32 = 0;
    var any = false;
    while (j < json.len and json[j] >= '0' and json[j] <= '9') : (j += 1) {
        v = v * 10 + @as(i32, json[j] - '0');
        any = true;
    }
    if (!any) return null;
    return if (neg) -v else v;
}

// ── Inference / training dispatch ──────────────────────────────────────────
fn handleInfer(msg: []const u8) void {
    var url_buf: [512]u8 = undefined;
    const url = readJsonString(msg, "url", &url_buf) orelse {
        logf_("infer: missing url field", .{});
        sendJson(
            \\{{"type":"error","message":"missing url"}}
        , .{});
        return;
    };

    const input_size: usize = @intCast(te_input_size());
    const input = te_input_ptr();
    const dst = @as([*]u8, @ptrCast(input))[0..input_size];

    const got = js_get_file_bin(url.ptr, url.len, dst.ptr, dst.len);
    if (got != input_size) {
        logf_("infer: fetched {d} bytes, expected {d}", .{ got, input_size });
        sendJson(
            \\{{"type":"error","message":"image size mismatch"}}
        , .{});
        return;
    }

    // The tutorial's main.cpp:106-122 stores pixels as (r-128, g-128, b-128).
    // We pass through the bytes as int8 directly (already signed-shifted in JS).
    te_run_inference();

    // Path C: 10-class mcunet-5fps head. Reads the codegen's int8 logits
    // and fp32-scaled scores for diagnostics; the demo's binary decision
    // uses only score indices 0/1. The optional binary-delta scores remain
    // padded to 10 entries to keep the JSON shape stable.
    var logits: [10]i8 = undefined;
    te_get_logits(&logits);
    var scores: [10]i32 = undefined;
    te_get_scores(&scores);
    var binary_scores: [10]i32 = undefined;
    te_get_binary_scores(&binary_scores);

    var full_best: usize = 0;
    for (1..10) |i| {
        if (scores[i] > scores[full_best]) full_best = i;
    }
    const scene_score = scores[0] + binary_scores[0];
    const person_score = scores[1] + binary_scores[1];
    const task_best: usize = if (person_score >= scene_score) 1 else 0;

    sendJson(
        \\{{"type":"infer_result","argmax":{d},"full_argmax":{d},"logits":[{d},{d},{d},{d},{d},{d},{d},{d},{d},{d}],"scores":[{d},{d},{d},{d},{d},{d},{d},{d},{d},{d}],"binary_scores":[{d},{d},{d},{d},{d},{d},{d},{d},{d},{d}]}}
    , .{
        task_best,        full_best,
        logits[0],        logits[1],
        logits[2],        logits[3],
        logits[4],        logits[5],
        logits[6],        logits[7],
        logits[8],        logits[9],
        scores[0],        scores[1],
        scores[2],        scores[3],
        scores[4],        scores[5],
        scores[6],        scores[7],
        scores[8],        scores[9],
        binary_scores[0], binary_scores[1],
        binary_scores[2], binary_scores[3],
        binary_scores[4], binary_scores[5],
        binary_scores[6], binary_scores[7],
        binary_scores[8], binary_scores[9],
    });
    setStatus("infer argmax={d} (full={d})", .{ task_best, full_best });
}

fn handleTrain(msg: []const u8) void {
    const arena_canary: i8 = 0x5a;
    var url_buf: [512]u8 = undefined;
    const url = readJsonString(msg, "url", &url_buf) orelse {
        sendJson(
            \\{{"type":"error","message":"missing url"}}
        , .{});
        return;
    };
    const label = readJsonInt(msg, "label") orelse {
        sendJson(
            \\{{"type":"error","message":"missing label"}}
        , .{});
        return;
    };

    const input_size: usize = @intCast(te_input_size());
    const input = te_input_ptr();
    const dst = @as([*]u8, @ptrCast(input))[0..input_size];

    te_arena_fill_canary(arena_canary);
    const got = js_get_file_bin(url.ptr, url.len, dst.ptr, dst.len);
    if (got != input_size) {
        sendJson(
            \\{{"type":"error","message":"image size mismatch"}}
        , .{});
        return;
    }

    // Train ONLY the fp32 prototype head over frozen pooled features.
    //
    // We tried running te_train_step alongside this (paper-faithful Path C
    // sparse update) but it collapses the classifier to "always scene":
    // the on-graph update mutates v8..v15 every step, so the pooled
    // features the prototype head accumulates into class means come from
    // a moving feature extractor. The centroids mix features from many
    // different "versions" of the backbone and lose discriminative power
    // (validation Δ values dropped from ±60 to ±0.5, all negative).
    //
    // Keeping the binary head as the sole trainer hits its centroid
    // asymptote in ~80 samples — validation predictions stop moving after
    // epoch 1 — but the asymptote is around 55% which is genuinely better
    // than the dual-path collapse to 50%. The on-graph entry point is
    // still smoke-tested (smoke_train_step) so the sparse update + QAS
    // pipeline stays verified; it's just unused at the demo-driver level
    // because it doesn't compose well with the prototype-head readout.
    te_train_binary_step(@intCast(label));
    const arena_touched = te_arena_count_touched(arena_canary);

    var mem: [8]i32 = undefined;
    te_get_memory(&mem);
    sendJson(
        \\{{"type":"train_result","label":{d},"sram_current":{d},"sram_peak":{d},"activation_sram":{d},"binary_head":{d},"arena_touched":{d}}}
    , .{ label, mem[3], mem[3], mem[0], mem[2], arena_touched });
    setStatus("train label={d}", .{label});
}

fn handleGetTrainDebug() void {
    // Path C: te_get_train_debug compares the 25 trainable v8..v15 tensors
    // against the snapshot taken in te_init and returns 16 ints — see
    // genModel.c for the layout. head_w_* / head_b_* are v15_weight /
    // v15_bias; block_w_* / block_b_* aggregate the v8..v14 backbone
    // updates. Plus the binary-head wrapper counters from bridge.c for
    // A/B comparison.
    var d: [16]i32 = undefined;
    te_get_train_debug(&d);
    var binary: [5]i32 = undefined;
    te_get_binary_debug(&binary);
    sendJson(
        \\{{"type":"train_debug","total_changed":{d},"total_abs":{d},"all_hash":{d},"head_w_changed":{d},"head_w_abs":{d},"head_w_hash":{d},"head_b_changed":{d},"head_b_abs":{d},"head_b_hash":{d},"block_w_changed":{d},"block_w_abs":{d},"block_w_hash":{d},"block_b_changed":{d},"block_b_abs":{d},"block_b_hash":{d},"snapshot_ready":{d},"binary_changed":{d},"binary_abs":{d},"binary_hash":{d},"binary_updates":{d},"binary_lr_scaled":{d}}}
    , .{
        d[0],      d[1],      d[2],
        d[3],      d[4],      d[5],
        d[6],      d[7],      d[8],
        d[9],      d[10],     d[11],
        d[12],     d[13],     d[14],
        d[15],     binary[0], binary[1],
        binary[2], binary[3], binary[4],
    });
}

fn handleReset() void {
    // Restore the QAS sparse-update tensors and re-zero the optional binary head.
    te_reset_weights();
    sendJson(
        \\{{"type":"reset_ack"}}
    , .{});
    setStatus("model reset", .{});
}

fn handleGetInputSig() void {
    // Return the first 16 bytes of getInput() as int8 values. The demo
    // verifier calls this right after each infer so it can confirm the
    // bytes JS handed off through js_get_file_bin actually landed at the
    // right place in wasm memory.
    var sig: [16]i8 = undefined;
    te_get_input_sig(&sig, sig.len);
    sendJson(
        \\{{"type":"input_sig","bytes":[{d},{d},{d},{d},{d},{d},{d},{d},{d},{d},{d},{d},{d},{d},{d},{d}]}}
    , .{
        sig[0], sig[1], sig[2],  sig[3],  sig[4],  sig[5],  sig[6],  sig[7],
        sig[8], sig[9], sig[10], sig[11], sig[12], sig[13], sig[14], sig[15],
    });
}

fn handleGetMemory() void {
    // Report the codegen's static memory budget plus the bridge.c
    // binary-head overhead plus the paper's Figure 10 variant comparison
    // (ft_full / ft_su / ft_sur). All values in bytes; the demo / floating
    // UI can convert to KB for display.
    var mem: [8]i32 = undefined;
    te_get_memory(&mem);
    sendJson(
        \\{{"type":"memory","peak_sram":{d},"model_flash":{d},"binary_head":{d},"train_sram_peak":{d},"input_bytes":{d},"ft_full_sram":{d},"ft_su_sram":{d},"ft_sur_sram":{d}}}
    , .{ mem[0], mem[1], mem[2], mem[3], mem[4], mem[5], mem[6], mem[7] });
}

fn handleGetPooledSig() void {
    // Return the first 16 of the 160 pooled features at buffer0[0] —
    // the activation vector that the generated head and optional fp32
    // binary-delta wrapper both operate on. If these don't
    // differ between very different inputs the backbone has collapsed
    // somewhere before global avg pool.
    var sig: [16]i8 = undefined;
    te_get_pooled_sig(&sig, sig.len);
    sendJson(
        \\{{"type":"pooled_sig","bytes":[{d},{d},{d},{d},{d},{d},{d},{d},{d},{d},{d},{d},{d},{d},{d},{d}]}}
    , .{
        sig[0], sig[1], sig[2],  sig[3],  sig[4],  sig[5],  sig[6],  sig[7],
        sig[8], sig[9], sig[10], sig[11], sig[12], sig[13], sig[14], sig[15],
    });
}

fn handleLoadInput(msg: []const u8) void {
    // Load bytes via js_get_file_bin and return the input signature WITHOUT
    // running invoke_inf. getInput() returns &buffer0[25600]; running
    // inference would overwrite parts of the input region with conv
    // intermediates, so the verifier needs to sample the sig before
    // invoke_inf to confirm the bytes JS sent landed correctly.
    var url_buf: [512]u8 = undefined;
    const url = readJsonString(msg, "url", &url_buf) orelse {
        sendJson(
            \\{{"type":"error","message":"missing url"}}
        , .{});
        return;
    };

    const input_size: usize = @intCast(te_input_size());
    const input = te_input_ptr();
    const dst = @as([*]u8, @ptrCast(input))[0..input_size];

    const got = js_get_file_bin(url.ptr, url.len, dst.ptr, dst.len);
    if (got != input_size) {
        sendJson(
            \\{{"type":"error","message":"image size mismatch"}}
        , .{});
        return;
    }

    var sig: [16]i8 = undefined;
    te_get_input_sig(&sig, sig.len);
    sendJson(
        \\{{"type":"input_sig","bytes":[{d},{d},{d},{d},{d},{d},{d},{d},{d},{d},{d},{d},{d},{d},{d},{d}]}}
    , .{
        sig[0], sig[1], sig[2],  sig[3],  sig[4],  sig[5],  sig[6],  sig[7],
        sig[8], sig[9], sig[10], sig[11], sig[12], sig[13], sig[14], sig[15],
    });
}

fn handleSetLr(msg: []const u8) void {
    // Only the optional binary-head LR is tunable from the demo UI.
    // We still accept legacy {"lr","blr"} keys but ignore them so older
    // demo code that sends them doesn't error; we reply with binary_lr
    // and dummy zeros for lr/blr to keep the envelope shape backwards-
    // compatible with the demo's set_lr_ack handler.
    if (readJsonFloat(msg, "binary_lr")) |v| te_set_binary_lr(v);
    var buf: [256]u8 = undefined;
    const json = std.fmt.bufPrint(&buf, "{{\"type\":\"set_lr_ack\",\"lr\":0,\"blr\":0,\"binary_lr\":{e}}}", .{te_get_binary_lr()}) catch return;
    js_ws_send(json.ptr, json.len);
    setStatus("set binary_lr={e}", .{te_get_binary_lr()});
}

// ── Headless smoke-test exports ────────────────────────────────────────────
// In normal operation the wasm boots via `run()` which spins up the message
// loop. These thin re-exports let a Node + WASI smoke test drive the te_*
// surface directly (see tools/smoke_pathc.mjs). They aren't called from
// the production JS host.

export fn smoke_init() i32 {
    return te_init();
}
export fn smoke_run_inference() void {
    te_run_inference();
}
export fn smoke_train_step(label: c_int) void {
    te_train_step(label);
}
export fn smoke_train_binary_step(label: c_int) void {
    te_train_binary_step(label);
}
export fn smoke_get_binary_scores(out: [*]i32) void {
    te_get_binary_scores(out);
}
export fn smoke_input_ptr() [*]i8 {
    return te_input_ptr();
}
export fn smoke_input_size() c_int {
    return te_input_size();
}
export fn smoke_input_width() c_int {
    return te_input_width();
}
export fn smoke_input_height() c_int {
    return te_input_height();
}
export fn smoke_input_channels() c_int {
    return te_input_channels();
}
export fn smoke_get_pooled_sig(out: [*]i8, n: c_int) void {
    te_get_pooled_sig(out, n);
}
export fn smoke_get_logits(out: [*]i8) void {
    te_get_logits(out);
}
export fn smoke_get_scores(out: [*]i32) void {
    te_get_scores(out);
}
export fn smoke_get_train_debug(out: [*]i32) void {
    te_get_train_debug(out);
}
export fn smoke_reset() void {
    te_reset_weights();
}

// ── Entry point invoked by JS ──────────────────────────────────────────────
export fn run() i32 {
    _ = te_init();

    var url_buf: [256]u8 = undefined;
    const url_len = js_get_ws_url(&url_buf, url_buf.len);
    const ws_url = url_buf[0..url_len];

    setStatus("connecting to {s} (W={d}x{d}x{d}, classes={d})", .{
        ws_url,
        te_input_width(),
        te_input_height(),
        te_input_channels(),
        te_num_classes(),
    });

    js_ws_connect(ws_url.ptr, ws_url.len);
    if (!waitState("connected")) {
        logf_("connect timeout", .{});
        return -1;
    }

    var agent_buf: [128]u8 = undefined;
    const agent_len = waitAgentId(&agent_buf);
    if (agent_len == 0) {
        logf_("no agent_id", .{});
        return -1;
    }
    setStatus("connected as {s}", .{agent_buf[0..agent_len]});

    sendJson(
        \\{{"type":"ready","module":"et-ws-zig-te-train1","input_w":{d},"input_h":{d},"input_c":{d},"classes":{d}}}
    , .{ te_input_width(), te_input_height(), te_input_channels(), te_num_classes() });

    var msg_buf: [2048]u8 = undefined;
    while (true) {
        const n = popMessage(&msg_buf);
        if (n == 0) break;
        const msg = msg_buf[0..n];

        var type_buf: [32]u8 = undefined;
        const mtype = readJsonString(msg, "type", &type_buf) orelse {
            logf_("dropped unparseable message", .{});
            continue;
        };

        if (std.mem.eql(u8, mtype, "infer")) {
            handleInfer(msg);
        } else if (std.mem.eql(u8, mtype, "train")) {
            handleTrain(msg);
        } else if (std.mem.eql(u8, mtype, "get_train_debug")) {
            handleGetTrainDebug();
        } else if (std.mem.eql(u8, mtype, "reset")) {
            handleReset();
        } else if (std.mem.eql(u8, mtype, "set_lr")) {
            handleSetLr(msg);
        } else if (std.mem.eql(u8, mtype, "get_input_sig")) {
            handleGetInputSig();
        } else if (std.mem.eql(u8, mtype, "get_pooled_sig")) {
            handleGetPooledSig();
        } else if (std.mem.eql(u8, mtype, "get_memory")) {
            handleGetMemory();
        } else if (std.mem.eql(u8, mtype, "load_input")) {
            handleLoadInput(msg);
        } else if (std.mem.eql(u8, mtype, "shutdown")) {
            setStatus("shutdown received", .{});
            break;
        } else {
            logf_("unknown type: {s}", .{mtype});
        }
    }

    js_ws_disconnect();
    setStatus("agent exited cleanly", .{});
    return 0;
}
