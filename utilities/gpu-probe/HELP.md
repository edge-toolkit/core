# Command-Line Help for `et-gpu-probe-server`

This document contains the help content for the `et-gpu-probe-server` command-line program.

**Command Overview:**

- [`et-gpu-probe-server`](#et-gpu-probe-server)

## `et-gpu-probe-server`

Collect GPU probe signatures into a registry over HTTPS

**Usage:** `et-gpu-probe-server [OPTIONS] --registry <REGISTRY> --cert <CERT> --key <KEY>`

### Options

- `--registry <REGISTRY>` -- The registry directory submissions are added to, such as `libs/gpu-probe/signatures`
- `--root <ROOT>` -- A directory of static files, such as a probe page, to serve at `/`.

  Absent, the server serves only the form, the endpoint and the registry: there is no page to serve.
- `--bind <BIND>` -- The address to listen on: `0.0.0.0` for other devices on the LAN to reach it

  Default value: `127.0.0.1`
- `--port <PORT>` -- The port to listen on

  Default value: `8444`
- `--cert <CERT>` -- The PEM certificate to serve, generated self-signed with the key when either is missing
- `--key <KEY>` -- The PEM private key of the certificate
- `--public-host <PUBLIC_HOST>` -- Another name to advertise the server at, which a generated certificate covers too
- `--http` -- Serve plain HTTP instead of HTTPS, which only a loopback `--bind` may.

  A browser treats a loopback origin as secure, so WebGPU still runs there.

  `--cert` and `--key` are not read.

## Environment variables

### `GITHUB_REPOSITORY`

Type: string. Default: empty.

The repository, `owner/name`.

### `GITHUB_RUN_ID`

Type: string. Default: empty.

The run's number.

### `GITHUB_SERVER_URL`

Type: string. Default: empty.

The server, such as `https://github.com`.

### `GPU_PROBE_DEVICE`

Type: string. Default: empty.

The machine a measurement names as its device, such as `Dell Precision 5570`, where it is given none itself.

### `GPU_PROBE_INTO`

Type: string. Default: empty.

A directory a measurement writes what it measured into as a file of submissions; none is written when empty.

### `IMAGEOS`

Type: string. Default: empty.

The runner image's name, such as `macos15` or `ubuntu24`, which a GitHub Actions runner sets as `ImageOS`.

### `IMAGEVERSION`

Type: string. Default: empty.

The runner image's build, such as `20261110.1`, which a GitHub Actions runner sets as `ImageVersion`.
