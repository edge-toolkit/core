{
    // Replaced by et-int-gen: every request goes through the host JS shim, so `self.http` never opens a
    // connection and has nothing to free. Calling its `deinit` would compile `std.http.Client`'s connection pool,
    // whose socket reader no longer builds for wasm32-freestanding.
    _ = self;
}
