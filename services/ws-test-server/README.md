# et-ws-test-server

An in-process edge-toolkit hub for integration tests. [`start`] runs the same ws hub, storage and modules services
the real `et-ws-server` mounts, on a free port and with a throwaway storage directory, and returns a [`TestServer`]
that stops the server and removes the directory when dropped. [`start_on`] does the same on a fixed port, for tests
that launch a separate process which must know the address up front.

It also carries the client-side helpers such tests keep needing, such as [`wait_for_connected_agents`], which blocks
until a given number of agents have registered with the hub.
