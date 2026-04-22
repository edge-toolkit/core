// zig-data1: replicates data1 workflow in Zig compiled to WASM.
// All browser I/O is provided by JS imports; Zig owns the workflow logic.

const std = @import("std");

extern fn js_log(ptr: [*]const u8, len: usize) void;
extern fn js_set_status(ptr: [*]const u8, len: usize) void;
extern fn js_ws_connect(url_ptr: [*]const u8, url_len: usize) void;
extern fn js_ws_send(ptr: [*]const u8, len: usize) void;
extern fn js_ws_disconnect() void;
extern fn js_ws_get_state(buf: [*]u8, max: usize) usize;
extern fn js_ws_get_agent_id(buf: [*]u8, max: usize) usize;
extern fn js_ws_pop_response(buf: [*]u8, max: usize) usize;
extern fn js_put_file(url_ptr: [*]const u8, url_len: usize, body_ptr: [*]const u8, body_len: usize) void;
extern fn js_get_file(url_ptr: [*]const u8, url_len: usize, buf: [*]u8, max: usize) usize;
extern fn js_sleep_ms(ms: u32) void;
extern fn js_get_ws_url(buf: [*]u8, max: usize) usize;
extern fn js_get_iso_timestamp(buf: [*]u8, max: usize) usize;

// Declared in src/util.c
extern fn byte_sum(buf: [*]const u8, len: usize) u8;

var heap: [64 * 1024]u8 = undefined;
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

fn wait_response(prefix: []const u8, buf: []u8) usize {
    var i: u32 = 0;
    while (i < 50) : (i += 1) {
        const n = js_ws_pop_response(buf.ptr, buf.len);
        if (n > 0 and std.mem.startsWith(u8, buf[0..n], prefix)) return n;
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

    // 1. Request store URL
    const store_msg = std.fmt.allocPrint(alloc,
        \\{{"type":"store_file","filename":"{s}"}}
    , .{filename}) catch return -1;
    defer alloc.free(store_msg);
    log("requesting store URL", .{});
    js_ws_send(store_msg.ptr, store_msg.len);

    var resp_buf: [512]u8 = undefined;
    const store_resp_len = wait_response("PUT to ", &resp_buf);
    if (store_resp_len == 0) {
        log("timed out waiting for store URL", .{});
        return -1;
    }
    const store_url = resp_buf[7..store_resp_len]; // strip "PUT to "
    log("storing data to {s}", .{store_url});
    set_status("zig-data1: storing data to {s}", .{store_url});
    js_put_file(store_url.ptr, store_url.len, content.ptr, content.len);

    // 2. Request fetch URL
    const fetch_msg = std.fmt.allocPrint(alloc,
        \\{{"type":"fetch_file","agent_id":"{s}","filename":"{s}"}}
    , .{ agent_id, filename }) catch return -1;
    defer alloc.free(fetch_msg);
    log("requesting fetch URL", .{});
    js_ws_send(fetch_msg.ptr, fetch_msg.len);

    const fetch_resp_len = wait_response("GET from ", &resp_buf);
    if (fetch_resp_len == 0) {
        log("timed out waiting for fetch URL", .{});
        return -1;
    }
    const fetch_url = resp_buf[9..fetch_resp_len]; // strip "GET from "
    log("fetching data from {s}", .{fetch_url});
    set_status("zig-data1: fetching data from {s}", .{fetch_url});

    var get_buf: [512]u8 = undefined;
    const got_len = js_get_file(fetch_url.ptr, fetch_url.len, &get_buf, get_buf.len);
    const got = get_buf[0..got_len];

    if (std.mem.eql(u8, got, content)) {
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
