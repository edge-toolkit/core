{
    // Replaced by et-int-gen: delegate to the rewritten `requestRawWithContentType`, which dispatches via the
    // host JS shim instead of `std.http.Client.fetch`. Every per-operation wrapper that declares headers funnels
    // through here. The host import carries no request headers, so `extra_headers` is dropped there exactly as
    // the client's default headers are.
    _ = extra_headers;
    return requestRawWithContentType(client, method, url, payload, content_type_value);
}
