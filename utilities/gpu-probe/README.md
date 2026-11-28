# et-gpu-probe-server

A server that collects GPU probe signatures into the `et-gpu-probe` registry, `libs/gpu-probe/signatures`, and the
probe page it serves. It serves HTTPS, since WebGPU needs a secure context and browsers treat every origin but a
loopback one as insecure over plain HTTP; `--http` serves plain HTTP on a loopback address alone.

## The probe page

`static/index.html` loads the probe sets built as WASM from `web/` and runs every one the browser can: the shader,
clear and fixed-function sets through WebGL 2, and every WebGPU set on each adapter WebGPU gives -- the
high-performance one, the low-power one and the software fallback. It shows each signature with a button that submits
it to the registry, under the GPU it ran on. The row it is seen at is filled in from what the browser reports, and can
be corrected before submitting. A Chromium browser on Android gives the phone's model code, such as `SM-G973F`, as the
device, and every Chromium browser gives the system's version; `?device=<machine>` and `?os_version=<version>` give
them instead, and `?submit` submits every set once probed.

`?memory=<MiB>` also walks the high-performance adapter's memory once the sets are probed, allocating until the
browser refuses or the walk holds that many MiB, and shows how far it went. Under headless Chrome on Linux the adapter
is SwiftShader unless Chrome is started with `--use-angle=vulkan --enable-features=Vulkan,VulkanFromANGLE
--disable-vulkan-surface`; SwiftShader reserves memory without backing it, so its walk always reaches the cap.

The tools that build the page, and every task here that builds, serves or drives it, are in the opt-in `webprobe`
mise env, so each is run with `-E webprobe`, once `mise -E webprobe install` has installed its tools. Serve it on this
machine, which builds the WASM first:

```bash
mise -E webprobe run gpu-probe-page
```

then open `http://localhost:8099/`. Chrome on Linux gives WebGPU only behind switches, and runs WebGL on whichever
ANGLE backend it is told to, so this opens it with WebGPU on and WebGL on `vulkan`, `gl` or `swiftshader`, serving the
page itself if nothing does yet; `--submit` drives it through chromedriver instead, submitting every set:

```bash
mise -E webprobe run gpu-probe-chrome vulkan --device "Dell Precision 5570" --submit
```

For phones and other machines, serve it over HTTPS on this machine or on the LAN:

```bash
mise -E webprobe run gpu-probe-server
```

```bash
mise -E webprobe run gpu-probe-server-lan
```

Then open one of the addresses it prints, and accept the self-signed certificate once in each browser. The
certificate covers localhost, each LAN address, and each address's `nip.io` name, which is a real domain for trying a
browser defence that treats a bare IP differently. `mise -E webprobe run build-gpu-probe-page` builds the WASM
alone, into `static/pkg`.

A phone's model code names it exactly, but not as it is sold. Google Play's
[supported devices list](https://storage.googleapis.com/play_public/supported_devices.html), also a
[CSV](https://storage.googleapis.com/play_public/supported_devices.csv), gives each code's brand and marketing name.
Two projects republish it as JSON: [bsthen/device-models](https://github.com/bsthen/device-models), keyed by model
code and rebuilt daily, and [pbakondy/android-device-list](https://github.com/pbakondy/android-device-list). Their
Apache-2.0 and MIT licences cover their own code; the data is Google's, which states no licence for it, so a name is
looked up there when curating a row rather than the list being copied into this repository.

## On a static host

A browser that cannot reach this machine -- Tor Browser, which sends everything through Tor -- can be probed from
the page deployed to a public static host such as Vercel. This assembles the page, its WASM, the files the server
serves beside it and `libs/gpu-probe/signatures.json` into `target/gpu-probe-site`, the directory to deploy:

```bash
mise -E webprobe run build-gpu-probe-site
```

With no server behind it, the page has no registry to add to, so instead of submit buttons it proposes what it found
as a pull request. It first holds each probe set against the JSON it was deployed with, as the server would, and says
whether the registry knows the device: by name when it records that device alone giving every answer the visit gave,
and otherwise which devices gave them all, or failing that which came closest. It leaves out what the registry already
records unless asked to send it too, and says how many sets were new signatures and which were known ones seen
somewhere new, such as on a new browser version. The rest go in plain-text files of submissions for the registry's
`submitted/` directory, which name each GPU's row once and give each probe set's values without their names. A file's
link opens GitHub's page for adding a file, filled in with it; GitHub forks the repository for a visitor who cannot
write to it, and opens a pull request when they press "Propose changes". A visit with two GPUs fits in one link; one
with more is split between GPUs across as many files as keep each link short enough for GitHub to open, each its own
pull request. Each file is named for its content, so proposing the same visit twice names a file the first proposal
already made.

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
and every curated entry as one document, and `/signatures.json` the same document as JSON, which the probe page holds
what it measured against to say whether the registry knows the device, as it does on a static host. `--root` serves a
probe page's files at `/`.

A row the registry already holds with every field the same, perhaps among more written by hand, records the sighting
already, so nothing is added for it. The day and the run's URL are not compared: the row keeps the first ones.

## Recording this machine

`gpu-probe` puts every probe set to each GPU adapter wgpu finds on this machine -- every GPU, and any software renderer
such as llvmpipe -- and to its CPU, and adds each sighting the repository's registry does not yet hold. The row is built
from what the device reports -- its vendor, name and backend, and the driver and API versions the graphics API beneath
wgpu gives -- and from the machine: the system's release and the CPU's model, the device `--device` names, the day, and
the wgpu and build target that measured it. Running it again adds nothing.

```bash
cargo run -p et-gpu-probe-server --bin gpu-probe -- --device "Dell Precision 5570"
```

On a GitHub Actions runner the device is the runner image and its build, and the row links the run. `--into <dir>`
writes the sightings as a file of submissions in that directory instead, for a registry that cannot be written to;
the `gpu-probe` workflow does that on every free runner image, and gathers what `nn_probe` in `et-ws-wasi-runner` and
the graphics-info case of `et-ws-web-runner`'s module tests propose the same way where `GPU_PROBE_INTO` names a
directory. Its `gpu-probe-submitted` artifact's files go in `submitted/`, for `mise run gpu-probe-fold`.

After collecting, regenerate the JSON the crate embeds:

```bash
mise run gen:gpu-probe-signatures
```
