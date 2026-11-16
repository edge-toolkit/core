# et-ws-test-server

An in-process edge-toolkit hub for integration tests. [`start`] runs the same ws hub, storage and modules services
the real `et-ws-server` mounts, on a free port and with a throwaway storage directory, and returns a [`TestServer`]
that stops the server and removes the directory when dropped. [`start_on`] does the same on a fixed port, for tests
that launch a separate process which must know the address up front.

It also carries the client-side helpers such tests keep needing, such as [`wait_for_connected_agents`], which blocks
until a given number of agents have registered with the hub. [`start_with`] starts a hub set up by [`HubOptions`]: its
workers, the most connections each holds, a second listener on `[::1]`, and routes of the caller's own.

## Benchmarks

`benches/hub.rs` holds one hub to a single CPU core and serves 25,000 agents from it: the time to connect and register
them all, then, with all of them connected and heartbeating as the native runners do, the hub thread's CPU time over
10 seconds, one more agent's connect, and an `et-alive` round trip. Record a machine's results with

```bash
mise run bench services/ws-test-server dell-precision-5570
```

which writes each benchmark's mean and 95% interval to `benches/results/<machine>-<os>.yaml`. Both ends of every
agent's socket are open in the one process, so the open-file limit must allow about 50,000.
