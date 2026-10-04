# et-websockify-service

A WebSocket-to-TCP relay for reaching the ws-server's own loopback HTTP port from a browser.

A browser WebAssembly runtime -- notably webR (R in the browser) -- can open a WebSocket but cannot open a raw TCP
socket. libcurl/httr2 compiled under Emscripten work around this by tunnelling their TCP bytes over a WebSocket and
expecting a websockify-style relay on the far end (see <https://emscripten.org/docs/porting/networking.html>). This
service is that relay.

It recognises two client shapes on the same `/websockify` route, by the first byte:

- **SOCKS5** (first byte `0x05`): webR's curl is configured to reach a SOCKS5 proxy, so the relay speaks just enough
  SOCKS5 (no-auth, CONNECT) to be that proxy. The requested CONNECT target is honoured only when it is loopback;
  every connection is then bridged to the single server-configured target (its own plain-HTTP port). A non-loopback
  CONNECT (e.g. curl's probe to the public r-universe proxy) is refused with a SOCKS5 error, so it never reaches the
  app server.
- **Direct byte stream** (anything else, e.g. a raw HTTP request): bridged straight to the same target.

Either way the target is fixed by the server (see [`configure`]), never taken from the client, so a browser cannot
point it at an arbitrary host (no SSRF / open proxy). It is a separate route from the agent hub's `/ws`: Emscripten
frames carry raw TCP bytes with no marker of their own, indistinguishable from the hub's binary-broadcast fallback,
so the two must never share one socket -- the separate path is the separation.
