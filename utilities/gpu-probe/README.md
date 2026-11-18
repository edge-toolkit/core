# et-gpu-probe-server

A server that collects GPU probe signatures into the `et-gpu-probe` registry, `libs/gpu-probe/signatures`, and the
probe page it serves. It serves HTTPS, since WebGPU needs a secure context and browsers treat every origin but a
loopback one as insecure over plain HTTP; `--http` serves plain HTTP on a loopback address alone.

## The probe page

`static/index.html` loads the probe sets built as WASM from `web/` and runs every one the browser can: the shader,
clear and fixed-function sets through WebGL 2, and all four WebGPU sets on each adapter WebGPU gives -- the
high-performance one, the low-power one and the software fallback. It shows each signature with a button that submits
it to the registry, under the GPU it ran on. The row it is seen at is filled in from what the browser reports, and can
be corrected before submitting; `?device=<machine>` fills in the device, and `?submit` submits every set once probed.
Serve it on this machine, which builds the WASM first:

```bash
mise run gpu-probe-page
```

then open `http://localhost:8099/`. Chrome on Linux gives WebGPU only behind switches, and runs WebGL on whichever
ANGLE backend it is told to, so this opens it with WebGPU on and WebGL on `vulkan`, `gl` or `swiftshader`, serving the
page itself if nothing does yet; `--submit` drives it through chromedriver instead, submitting every set:

```bash
mise run gpu-probe-chrome vulkan --device "Dell Precision 5570" --submit
```

For phones and other machines, serve it over HTTPS on this machine or on the LAN:

```bash
mise run gpu-probe-server
```

```bash
mise run gpu-probe-server-lan
```

Then open one of the addresses it prints, and accept the self-signed certificate once in each browser. The
certificate covers localhost, each LAN address, and each address's `nip.io` name, which is a real domain for trying a
browser defence that treats a bare IP differently. `mise run build-gpu-probe-page` builds the WASM alone, into
`static/pkg`.

## On a static host

A browser that cannot reach this machine -- Tor Browser, which sends everything through Tor -- can be probed from
the page deployed to a public static host such as Vercel. This assembles the page, its WASM and the files the server
serves beside it into `target/gpu-probe-site`, the directory to deploy:

```bash
mise run build-gpu-probe-site
```

With no server behind it, the page has no registry to add to, so instead of submit buttons it proposes what it found
as a pull request. It shows links to GitHub's page for adding a file, each filled in with a file of submissions for
the registry's `submitted/` directory; GitHub forks the repository for a visitor who cannot write to it, and opens the
pull request when they press "Propose changes". A visit takes as many files, and links, as keep each link short
enough for GitHub to open. Each file is named for its content, so proposing the same visit twice names a file the
first proposal already made.

A pull request of a submitted file leaves `libs/gpu-probe/signatures.json` alone, since the JSON is generated from the
curated entries only, and the tests check every submitted file is one the registry would take. Once such pull
requests are merged, file what they proposed and regenerate the JSON on `main`, then commit the result:

```bash
mise run gpu-probe-fold
```

## Submitting

`POST /register` takes one JSON object: the `interface` the signature was measured through, a `label`, the
`results` keyed by name in output order -- or `blocked: true` and a `reason` for an interface a browser withheld --
and the `seen` row of where it was measured, with the fields the registry's header lists:

```json
{
  "interface": "webgpu-clear",
  "label": "Intel Iris Xe (ADL GT2) through Mesa's Vulkan driver on Linux -- wgpu 29",
  "results": { "unorm8_quarter": "0000000a", "unorm8_half": "0000000a" },
  "seen": {
    "browser": "none",
    "os": "linux",
    "cpu_arch": "x86_64",
    "gpu": "intel",
    "backend": "vulkan",
    "window": "none",
    "driver_version": "26.1.6"
  }
}
```

A signature the registry has through that interface gains the row, unless it holds the same row already; anything
else becomes a new entry at the end of the interface's file under `curated/`, which is made if the interface has none
yet. The file is edited as text, so its comments and every other entry are left as they were. `/submit` posts each
line pasted into it, as JSON submissions one per line. `/signatures.yaml` serves the registry as it stands, its header
and every curated entry as one document, and `--root` serves a probe page's files at `/`.

A row the registry already holds with every field the same, perhaps among more written by hand, records the sighting
already, so nothing is added for it.

## Recording this machine

`gpu-probe` takes no arguments. It puts every probe set to each GPU adapter wgpu finds on this machine -- every GPU,
and any software renderer such as llvmpipe -- and to its CPU, and adds each sighting the repository's registry does
not yet hold, in a row built from what the device reports: its
vendor, name and backend, and the driver and API versions the graphics API beneath wgpu gives. Running it again adds
nothing.

```bash
cargo run -p et-gpu-probe-server --bin gpu-probe
```

After collecting, regenerate the JSON the crate embeds:

```bash
mise run gen:gpu-probe-signatures
```
