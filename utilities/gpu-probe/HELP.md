# Command-Line Help for `et-gpu-probe-server`

This document contains the help content for the `et-gpu-probe-server` command-line program.

**Command Overview:**

- [`et-gpu-probe-server`](#et-gpu-probe-server)

## `et-gpu-probe-server`

Collect GPU probe signatures into a registry over HTTPS

**Usage:** `et-gpu-probe-server [OPTIONS] --registry <REGISTRY> --cert <CERT> --key <KEY>`

### Options

- `--registry <REGISTRY>` -- The registry file submissions are added to, such as `libs/gpu-probe/signatures.yaml`
- `--root <ROOT>` -- A directory of static files, such as a probe page, to serve at `/`.

  Absent, the server serves only the form, the endpoint and the registry: there is no page to serve.
- `--bind <BIND>` -- The address to listen on: `0.0.0.0` for other devices on the LAN to reach it

  Default value: `127.0.0.1`
- `--port <PORT>` -- The port to listen on

  Default value: `8443`
- `--cert <CERT>` -- The PEM certificate to serve, generated self-signed with the key when either is missing
- `--key <KEY>` -- The PEM private key of the certificate
- `--public-host <PUBLIC_HOST>` -- Another name to advertise the server at, which a generated certificate covers too
- `--http` -- Serve plain HTTP instead of HTTPS, which only a loopback `--bind` may.

  A browser treats a loopback origin as secure, so WebGPU still runs there.

  `--cert` and `--key` are not read.
