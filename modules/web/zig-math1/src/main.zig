// zig-math1: storage-driven federated-averaging (FedAvg) demo in a Zig wasm module.
// Waits for the broadcast math1-input pointer, reads the input JSON (client datasets +
// hyperparameters) from ws-server storage through the worker shim's REST relay, runs the kernel --
// only + - * / on f64 in a fixed evaluation order, bit-identical to the other math1 twins -- and
// stores the global model to math1-output.json in its own bucket, where the test harness reads and
// verifies it. All browser I/O is provided by JS imports, mirroring zig-data1's worker shim.

const std = @import("std");

extern fn js_log(ptr: [*]const u8, len: usize) void;
extern fn js_set_status(ptr: [*]const u8, len: usize) void;
extern fn js_ws_connect(url_ptr: [*]const u8, url_len: usize) void;
extern fn js_ws_disconnect() void;
extern fn js_ws_get_state(buf: [*]u8, max: usize) usize;
extern fn js_ws_get_agent_id(buf: [*]u8, max: usize) usize;
extern fn js_ws_get_input(buf: [*]u8, max: usize) usize;
extern fn js_sleep_ms(ms: u32) void;
extern fn js_get_ws_url(buf: [*]u8, max: usize) usize;
extern fn js_rest_request(
    method_ptr: [*]const u8,
    method_len: usize,
    url_ptr: [*]const u8,
    url_len: usize,
    body_ptr: [*]const u8,
    body_len: usize,
    buf: [*]u8,
    max: usize,
) i32;

var heap: [64 * 1024]u8 = undefined;
var fba = std.heap.FixedBufferAllocator.init(&heap);
const alloc = fba.allocator();

const Math1Input = struct {
    clients: [][][2]f64,
    rounds: u32,
    epochs: u32,
    learning_rate: f64,
};

const Model = struct { weight: f64, bias: f64 };

fn log(comptime fmt: []const u8, args: anytype) void {
    const msg = std.fmt.allocPrint(alloc, "[zig-math1] " ++ fmt, args) catch return;
    defer alloc.free(msg);
    js_log(msg.ptr, msg.len);
}

fn set_status(comptime fmt: []const u8, args: anytype) void {
    const msg = std.fmt.allocPrint(alloc, fmt, args) catch return;
    defer alloc.free(msg);
    js_set_status(msg.ptr, msg.len);
}

fn wait_state(want: []const u8) bool {
    var buf: [32]u8 = undefined;
    var i: u32 = 0;
    while (i < 100) : (i += 1) {
        const n = js_ws_get_state(&buf, buf.len);
        if (std.mem.eql(u8, buf[0..n], want)) return true;
        js_sleep_ms(100);
    }
    return false;
}

fn wait_agent_id(buf: []u8) usize {
    var i: u32 = 0;
    while (i < 100) : (i += 1) {
        const n = js_ws_get_agent_id(buf.ptr, buf.len);
        if (n > 0) return n;
        js_sleep_ms(100);
    }
    return 0;
}

// The main-thread shim serialises the captured math1-input pointer as "bucket\nfilename".
fn wait_input_pointer(buf: []u8) usize {
    var i: u32 = 0;
    while (i < 100) : (i += 1) {
        const n = js_ws_get_input(buf.ptr, buf.len);
        if (n > 0) return n;
        js_sleep_ms(100);
    }
    return 0;
}

// Runs the FedAvg simulation on the fetched input and returns the final global model.
fn fed_avg(input: Math1Input) Model {
    var weight: f64 = 0.0;
    var bias: f64 = 0.0;
    var total_samples: f64 = 0.0;
    for (input.clients) |samples| {
        total_samples += @as(f64, @floatFromInt(samples.len));
    }
    var round: u32 = 0;
    while (round < input.rounds) : (round += 1) {
        var merged_weight: f64 = 0.0;
        var merged_bias: f64 = 0.0;
        for (input.clients) |samples| {
            const count: f64 = @floatFromInt(samples.len);
            var client_weight = weight;
            var client_bias = bias;
            var epoch: u32 = 0;
            while (epoch < input.epochs) : (epoch += 1) {
                var grad_weight: f64 = 0.0;
                var grad_bias: f64 = 0.0;
                for (samples) |sample| {
                    const residual = client_weight * sample[0] + client_bias - sample[1];
                    grad_weight += residual * sample[0];
                    grad_bias += residual;
                }
                client_weight -= input.learning_rate * (2.0 * grad_weight / count);
                client_bias -= input.learning_rate * (2.0 * grad_bias / count);
            }
            merged_weight += client_weight * count;
            merged_bias += client_bias * count;
        }
        weight = merged_weight / total_samples;
        bias = merged_bias / total_samples;
    }
    return .{ .weight = weight, .bias = bias };
}

export fn run() i32 {
    var url_buf: [256]u8 = undefined;
    const url_len = js_get_ws_url(&url_buf, url_buf.len);
    const ws_url = url_buf[0..url_len];

    log("entered run()", .{});
    set_status("zig-math1: entered run()", .{});

    js_ws_connect(ws_url.ptr, ws_url.len);

    if (!wait_state("connected")) {
        log("timed out waiting for connection", .{});
        return -1;
    }

    var agent_buf: [128]u8 = undefined;
    const agent_len = wait_agent_id(&agent_buf);
    if (agent_len == 0) {
        log("timed out waiting for agent_id", .{});
        return -1;
    }
    const agent_id = agent_buf[0..agent_len];
    log("connected as {s}", .{agent_id});
    set_status("zig-math1: connected as {s}", .{agent_id});

    set_status("zig-math1: waiting for the math1-input pointer broadcast", .{});
    var pointer_buf: [512]u8 = undefined;
    const pointer_len = wait_input_pointer(&pointer_buf);
    if (pointer_len == 0) {
        log("timed out waiting for the math1-input pointer", .{});
        return -1;
    }
    const pointer = pointer_buf[0..pointer_len];
    const newline = std.mem.indexOfScalar(u8, pointer, '\n') orelse {
        log("malformed input pointer: {s}", .{pointer});
        return -1;
    };
    const bucket = pointer[0..newline];
    const filename = pointer[newline + 1 ..];

    const input_url = std.fmt.allocPrint(alloc, "/storage/{s}/{s}", .{ bucket, filename }) catch return -1;
    defer alloc.free(input_url);
    set_status("zig-math1: reading input from {s}", .{input_url});
    var input_buf: [4096]u8 = undefined;
    const input_len = js_rest_request("GET", 3, input_url.ptr, input_url.len, "", 0, &input_buf, input_buf.len);
    if (input_len < 0) {
        log("input GET failed", .{});
        return -1;
    }
    const input_bytes = input_buf[0..@intCast(input_len)];

    const parsed = std.json.parseFromSlice(Math1Input, alloc, input_bytes, .{}) catch {
        log("input JSON parse failed", .{});
        return -1;
    };
    defer parsed.deinit();
    const input = parsed.value;

    set_status(
        "zig-math1: running FedAvg - {d} clients x {d} rounds x {d} local epochs",
        .{ input.clients.len, input.rounds, input.epochs },
    );
    const model = fed_avg(input);
    log("global model weight={d} bias={d}", .{ model.weight, model.bias });
    set_status("zig-math1: global model weight={d} bias={d}", .{ model.weight, model.bias });

    const output = std.fmt.allocPrint(
        alloc,
        "{{\"module\":\"zig-math1\",\"weight\":{d},\"bias\":{d}}}",
        .{ model.weight, model.bias },
    ) catch return -1;
    defer alloc.free(output);
    const output_url = std.fmt.allocPrint(alloc, "/storage/{s}/math1-output.json", .{agent_id}) catch return -1;
    defer alloc.free(output_url);
    var put_buf: [256]u8 = undefined;
    const put_len =
        js_rest_request("PUT", 3, output_url.ptr, output_url.len, output.ptr, output.len, &put_buf, put_buf.len);
    if (put_len < 0) {
        log("output PUT failed", .{});
        return -1;
    }
    set_status("zig-math1: stored the global model to {s}", .{output_url});

    js_sleep_ms(2000);
    js_ws_disconnect();
    log("workflow complete", .{});
    set_status("zig-math1: workflow complete", .{});
    return 0;
}
