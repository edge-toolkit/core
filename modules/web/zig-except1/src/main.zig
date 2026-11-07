// zig-except1: demonstrates exception handling across the C++ layer of a Zig wasm module.
// The C++ TU (src/exceptions.cpp) documents the exception models available on wasm32-freestanding and
// carries the minimal runtime its `catch (...)`-only model needs. This Zig side only ever sees status
// codes: no exception may unwind through Zig frames, so every C++ entry point catches what it throws.
// All browser I/O is provided by JS imports, mirroring zig-data1's worker shim (minus its REST path).

const std = @import("std");

extern fn js_log(ptr: [*]const u8, len: usize) void;
extern fn js_set_status(ptr: [*]const u8, len: usize) void;
extern fn js_ws_connect(url_ptr: [*]const u8, url_len: usize) void;
extern fn js_ws_disconnect() void;
extern fn js_ws_get_state(buf: [*]u8, max: usize) usize;
extern fn js_ws_get_agent_id(buf: [*]u8, max: usize) usize;
extern fn js_sleep_ms(ms: u32) void;
extern fn js_get_ws_url(buf: [*]u8, max: usize) usize;

// Declared in src/exceptions.cpp; returns the quotient, or -1 when the divide throws (caught in C++).
extern fn try_divide(num: i32, den: i32) i32;

var heap: [16 * 1024]u8 = undefined;
var fba = std.heap.FixedBufferAllocator.init(&heap);
const alloc = fba.allocator();

fn log(comptime fmt: []const u8, args: anytype) void {
    const msg = std.fmt.allocPrint(alloc, "[zig-except1] " ++ fmt, args) catch return;
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

export fn run() i32 {
    var url_buf: [256]u8 = undefined;
    const url_len = js_get_ws_url(&url_buf, url_buf.len);
    const ws_url = url_buf[0..url_len];

    log("entered run()", .{});
    set_status("zig-except1: entered run()", .{});

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
    set_status("zig-except1: connected as {s}", .{agent_id});

    const quotient = try_divide(84, 4);
    log("try_divide(84, 4) = {d} (no throw)", .{quotient});
    set_status("zig-except1: try_divide(84, 4) = {d} (no throw)", .{quotient});

    const caught = try_divide(1, 0);
    log("try_divide(1, 0) = {d} (throw caught in C++)", .{caught});
    set_status("zig-except1: try_divide(1, 0) = {d} (throw caught in C++)", .{caught});

    if (quotient == 21 and caught == -1) {
        log("VERIFICATION SUCCESS - C++ throw/catch behaved as expected!", .{});
        set_status("zig-except1: VERIFICATION SUCCESS - C++ throw/catch behaved as expected!", .{});
    } else {
        log("VERIFICATION FAILURE - unexpected results!", .{});
        set_status("zig-except1: VERIFICATION FAILURE - unexpected results!", .{});
        js_ws_disconnect();
        return -1;
    }

    js_sleep_ms(2000);
    js_ws_disconnect();
    log("workflow complete", .{});
    set_status("zig-except1: workflow complete", .{});
    return 0;
}
