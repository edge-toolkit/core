
/// Host-provided HTTP transport. The JS shim implements this against
/// browser `fetch()` (via SharedArrayBuffer + Atomics so this looks
/// synchronous to Zig). Returns the number of bytes written to
/// `response_buf`, or a negative value on transport failure /
/// non-2xx status.
extern fn js_rest_request(
    method_ptr: [*]const u8,
    method_len: usize,
    url_ptr: [*]const u8,
    url_len: usize,
    body_ptr: [*]const u8,
    body_len: usize,
    response_buf: [*]u8,
    response_max: usize,
) i32;
