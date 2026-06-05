{
    // Replaced by et-int-gen: dispatch via the host JS shim instead of
    // `std.http.Client.fetch`, which can't reach the network from
    // browser wasm. The JS side proxies to `fetch()` via
    // SharedArrayBuffer + Atomics so this stays synchronous in Zig.
    const allocator = client.allocator;
    const method_str = @tagName(method);
    const body_slice = payload orelse "";
    const response_buf = try allocator.alloc(u8, 64 * 1024);
    const written = js_rest_request(
        method_str.ptr, method_str.len,
        url.ptr, url.len,
        body_slice.ptr, body_slice.len,
        response_buf.ptr, response_buf.len,
    );
    if (written < 0) {
        allocator.free(response_buf);
        return error.RequestFailed;
    }
    const n: usize = @intCast(written);
    const body = try allocator.realloc(response_buf, n);
    return .{ .allocator = allocator, .status = .ok, .body = body };
}
