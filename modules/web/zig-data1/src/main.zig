// zig-data1: replicates data1 workflow in Zig compiled to WASM.
// All browser I/O is provided by JS imports; Zig owns the workflow logic.
// HTTP goes through the generated `et_rest_client` typed client (which
// bottoms out in a single `extern fn js_rest_request` import implemented
// by the worker shim via SharedArrayBuffer + Atomics).

const std = @import("std");
const rest = @import("et_rest_client");

extern fn js_log(ptr: [*]const u8, len: usize) void;
extern fn js_set_status(ptr: [*]const u8, len: usize) void;
extern fn js_ws_connect(url_ptr: [*]const u8, url_len: usize) void;
extern fn js_ws_disconnect() void;
extern fn js_ws_get_state(buf: [*]u8, max: usize) usize;
extern fn js_ws_get_agent_id(buf: [*]u8, max: usize) usize;
extern fn js_sleep_ms(ms: u32) void;
extern fn js_get_ws_url(buf: [*]u8, max: usize) usize;
extern fn js_get_iso_timestamp(buf: [*]u8, max: usize) usize;

// Declared in src/util.c
extern fn byte_sum(buf: [*]const u8, len: usize) u8;

// Declared in src/util.cpp
extern fn fnv1a_hash(buf: [*]const u8, len: usize) u32;

// Bumped from 64K because the REST client allocates a 64K response buffer
// per request and the workflow runs several round-trips before completing.
var heap: [256 * 1024]u8 = undefined;
var fba = std.heap.FixedBufferAllocator.init(&heap);
const alloc = fba.allocator();

fn log(comptime fmt: []const u8, args: anytype) void {
    const msg = std.fmt.allocPrint(alloc, "[zig-data1] " ++ fmt, args) catch return;
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
    set_status("zig-data1: entered run()", .{});

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
    set_status("zig-data1: connected as {s}", .{agent_id});

    const filename = "test_data.txt";

    var ts_buf: [64]u8 = undefined;
    const ts_len = js_get_iso_timestamp(&ts_buf, ts_buf.len);
    const timestamp = ts_buf[0..ts_len];

    const content = std.fmt.allocPrint(alloc, "Hello from zig-data1 at {s}!", .{timestamp}) catch return -1;
    defer alloc.free(content);

    const cksum = byte_sum(content.ptr, content.len);
    log("content checksum (byte_sum from C): {d}", .{cksum});

    const hash = fnv1a_hash(content.ptr, content.len);
    log("content hash (fnv1a_hash from C++): {x:0>8}", .{hash});

    // The REST client targets the same origin we were served from, so an
    // empty base_url leaves it with relative paths like `/storage/{id}/{f}`
    // -- the browser resolves those against the page origin via fetch().
    var client = rest.Client.init(alloc, undefined, "");
    defer client.deinit();

    log("storing data to /storage/{s}/{s}", .{ agent_id, filename });
    set_status("zig-data1: storing data to /storage/{s}/{s}", .{ agent_id, filename });
    rest.put_file(&client, agent_id, filename, content) catch {
        log("put_file failed", .{});
        return -1;
    };

    log("fetching data from /storage/{s}/{s}", .{ agent_id, filename });
    set_status("zig-data1: fetching data from /storage/{s}/{s}", .{ agent_id, filename });
    var raw = rest.get_fileRaw(&client, agent_id, filename) catch {
        log("get_file failed", .{});
        return -1;
    };
    defer raw.deinit();

    if (std.mem.eql(u8, raw.body, content)) {
        log("VERIFICATION SUCCESS - data matches!", .{});
        set_status("zig-data1: VERIFICATION SUCCESS - data matches!", .{});
    } else {
        log("VERIFICATION FAILURE - data mismatch!", .{});
        set_status("zig-data1: VERIFICATION FAILURE - data mismatch!", .{});
        js_ws_disconnect();
        return -1;
    }

    js_sleep_ms(2000);
    js_ws_disconnect();
    log("workflow complete", .{});
    set_status("zig-data1: workflow complete", .{});
    return 0;
}
