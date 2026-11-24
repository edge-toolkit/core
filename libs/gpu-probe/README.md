# et-gpu-probe

Numeric and fixed-function probes that tell one GPU's arithmetic from another's, the registry of every signature they
have been seen to give, and, with the `native` feature, the driver that puts them to a GPU through wgpu.

## Collecting signatures

A machine's GPUs are reached through several mechanisms, and each gives signatures of its own, filed under
`signatures/curated/` by interface. Probe every one of them, on every GPU, and on the CPU where a mechanism falls back
to it: a software renderer such as WARP, llvmpipe or SwiftShader, or ONNX Runtime on the CPU, is a signature too.

### From a clone, on this computer

Install the tools first; everything below runs through mise, and `et-ws-wasi-runner` and its ONNX Runtime build only
with mise's tools:

```bash
MISE_ENV=rust mise install
```

Name the machine the same way in every step, by its make and model, so its rows agree.

1. **wgpu directly, and the CPU.** `gpu-probe` puts every probe set to each adapter wgpu finds through Vulkan, Metal
   and Direct3D 12, software renderers included, and the `host` set to the CPU, adding what the registry lacks:

   ```bash
   mise exec -- cargo run -p et-gpu-probe-server --bin gpu-probe -- --device "HP EliteBook 645 G10"
   ```

2. **Every browser, as WASM.** The probe page runs WebGL 1 and 2 on the GPU the browser picks, WebGPU on each
   adapter it offers (high-performance, low-power and its software fallback), and the canvas fingerprints. On Linux
   and macOS, run each installed browser in turn, and Chrome on Linux on each ANGLE backend:

   ```bash
   mise run gpu-probe-browser firefox --device "Dell Precision 5570" --submit
   mise run gpu-probe-chrome vulkan --device "Dell Precision 5570" --submit
   ```

   On Windows neither task runs, so serve the page with `mise run gpu-probe-page` and open
   `http://localhost:8099/?submit&device=<machine>` in each browser from a fresh profile (`--user-data-dir` for
   Chrome and Edge, `--no-remote --profile` with `.mise/gpu-probe-browser.user.js` as its `user.js` for Firefox), with
   `&browser=edge&browser_version=<version>` for Edge, which reports Chromium's. Open Chrome and Edge once with each
   `--use-angle=` of `d3d11`, `d3d11on12`, `d3d11-warp`, `gl`, `vulkan` and `swiftshader` (the last with
   `--enable-unsafe-swiftshader`), each of which runs WebGL through another API or renderer, once with
   `--enable-unsafe-webgpu --enable-unsafe-swiftshader`, without which they give WebGPU no fallback adapter, and once
   in a private window (`--incognito`, `--inprivate`, Firefox's `-private-window`, with `&window=private`).
   On a machine with two GPUs, add `--force_high_performance_gpu` for WebGL on the other one.

3. **`wasi:nn`.** ONNX Runtime behind `et-ws-wasi-runner`, under each execution target; without the `cuda` feature
   every target runs on the CPU. It proposes what it measured as a file in `signatures/submitted/`:

   ```bash
   GPU_PROBE_DEVICE="HP EliteBook 645 G10" GPU_PROBE_INTO=libs/gpu-probe/signatures/submitted \
     mise exec -- cargo test -p et-ws-wasi-runner --test nn_probe -- --nocapture
   ```

   On Linux with an NVIDIA GPU, run it again with `MISE_ENV=rust,cuda` and `--features cuda`, as CONTRIBUTING.md
   describes.

4. **Deno.** The graphics-info module puts the six WebGPU probe sets to each adapter Deno's WebGPU gives, under
   `et-ws-web-runner`, and proposes them the same way:

   ```bash
   mise run build-ws-graphics-info-module
   GPU_PROBE_DEVICE="HP EliteBook 645 G10" GPU_PROBE_INTO=libs/gpu-probe/signatures/submitted \
     mise exec -- cargo test -p et-ws-web-runner --test modules graphics_info -- --nocapture
   ```

   On Windows, run both under `MISE_ENV=rust,msvc`, after `mise run prefetch:msvc` stages the portable MSVC
   toolchain: the default gnullvm target has no V8 prebuilt, and `MISE_ENV=mingw`'s gnu link of the msvc one stops
   on v8 150.4.0 with `undefined symbol: v8__V8__EnableWebAssemblyTrapHandler`. A clone that has moved since it was
   staged needs the task run again, as `target/msvc/msvc.env` names the toolchain by its absolute path. On an HP
   EliteBook 645 G10's AMD Radeon (TM) Graphics, Deno's WebGPU refuses the module's second `requestDevice` with
   `OperationError: Not enough memory left.`, so no set is measured there yet.

5. **File and commit.** File what was proposed into `curated/` and regenerate the JSON the crate embeds, then check
   the rows -- a browser that blanks or masks its GPU, as Firefox does, is recorded as the GPU it ran on, with what it
   reported in `reported` -- and commit:

   ```bash
   mise run gpu-probe-fold
   ```

### From other devices nearby

Phones, tablets and other machines on the same network submit to this clone's registry through the HTTPS server:

```bash
mise run gpu-probe-server-lan
```

Open one of the addresses it prints in each browser on the device, accept the self-signed certificate, and submit each
row, or add `?submit&device=<model>` to submit every set at once. Then file and commit as above.

### Without a clone

Open the probe page hosted at <https://edge-toolkit-core.vercel.app/>, the repository's homepage, in each browser.
With no registry behind it, it holds what it measured against the registry it was deployed with, and proposes what is
new as a pull request of a file for `signatures/submitted/`, one link per file, or as a file to download and email.
A maintainer then files it with `mise run gpu-probe-fold`.

## Benchmarks

`benches/gpu_probe.rs` measures every operation the crate offers: building each probe set and its exact answers, the
texture formats' conversions, reading a signature through each interface, the registry, and opening the GPUs and
putting every probe set to each adapter the machine has. Record a machine's results with

```bash
mise run bench libs/gpu-probe dell-precision-5570
```

naming the machine as lowercase words joined by `-`. That runs the bench and writes
`benches/results/<machine>-<os>.yaml`, the OS read from the compiler's host, since the same machine gives other
results under another OS through its other drivers. It records the machine, OS, host triple, compiler and date, and
each benchmark's mean time with its 95% confidence interval, in nanoseconds, in Bencher Metric Format. criterion's raw
samples stay under `target/`.

### Dell Precision 5570, Linux

An Intel Core i7-12800H (14 cores, 20 threads) with 32 GB, on Pop!_OS 24.04 and Linux 7.2.15, built by rustc 1.100.0.
Its GPUs are an NVIDIA RTX A1000 Laptop GPU on driver 595.104.02 and the Iris Xe (Alder Lake GT2) integrated in the
CPU on Mesa 26.1.6, each through Vulkan, beside Mesa's llvmpipe software renderer. Recorded on 2026-11-13, on an
otherwise idle machine.

Putting each probe set to each adapter, through Vulkan: the mean time per call, with the 95% interval's width.

| Probe set                  | NVIDIA RTX A1000   | Intel Iris Xe (ADL GT2) | llvmpipe (LLVM 20.1.2) |
| -------------------------- | ------------------ | ----------------------- | ---------------------- |
| `signature`                | 1.1 ms +/- 0.13 ms | 968 us +/- 50 us        | 1.5 ms +/- 0.03 ms     |
| `combined_signature`       | 88 us +/- 8 us     | 169 us +/- 34 us        | 135 us +/- 9 us        |
| `clear_signature`          | 313 us +/- 24 us   | 612 us +/- 104 us       | 1.2 ms +/- 0.04 ms     |
| `fixed_function_signature` | 2.4 ms +/- 0.10 ms | 2.1 ms +/- 0.07 ms      | 21.5 ms +/- 25.8 ms    |
| `report`                   | 6.8 us +/- 0.06 us | 6.5 us +/- 0.04 us      | 51.7 us +/- 0.3 us     |

Opening the default adapter takes 93 ms, and opening every adapter 824 ms; each compiles the probe set's shaders for
the device. The combined shader is the fastest way to take the numeric signature on every adapter, since it is one
pipeline rather than sixteen. llvmpipe's fixed-function set is the one wide interval: a software renderer drawing on
the CPU stalls now and then, so its mean is less telling than the rest.

The rest runs on the CPU alone:

| Operation                                                        | Mean per call                  |
| ---------------------------------------------------------------- | ------------------------------ |
| `shaders` (the sixteen WGSL probe shaders)                       | 4.2 us                         |
| `combined_shader`                                                | 1.2 us                         |
| `fragment_shaders` (the GLSL ES set)                             | 482 ns                         |
| a fixed-function probe's WGSL or GLSL                            | 0.9 to 1.4 us                  |
| `reference`, `host`                                              | 7 ns, 41 ns                    |
| `clear_reference`, `fixed_function_reference`                    | 78 ns, 147 ns                  |
| `Format::encode`, `decode` (the sRGB format slowest)             | 4 to 37 ns                     |
| `Format::texel`, `red`                                           | 18 to 20 ns, 1.3 ns            |
| `Signature::fingerprint`, `results`, `traits`                    | 0.46 to 0.87 us                |
| `Signature::describe`, `class`                                   | 1.4 to 2.3 us, 0.43 to 0.71 us |
| `Signature::identify` (looking the signature up in the registry) | 1.8 to 2.8 us                  |
| `Signature::seen` (every row the registry holds for it)          | 1.8 to 3.8 us                  |
| `every_sighting` (every row of the registry)                     | 31 us                          |
| `Sighting::read`, `read_entry` (three rows)                      | 152 ns, 488 ns                 |
