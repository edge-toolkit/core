# et-gpu-probe

Numeric and fixed-function probes that tell one GPU's arithmetic from another's, the registry of every signature they
have been seen to give, and, with the `native` feature, the driver that puts them to a GPU through wgpu.

## The registry records everything

The registry exists to record every variation of CPU and GPU behaviour there is: every chip, driver, driver version
and driver setting, backend, operating system, browser and browser version, window mode and privacy defence, and every
signature each combination gives -- as accurately as it can be measured, and with as much metadata about where it was
measured as can be gathered. Nothing is too ordinary to record and nothing is too surprising to keep.

A signature is what the whole stack answered, not what the silicon alone would. A driver that answers a clear from
the CPU instead of the GPU, a browser that rounds or randomises a read-back, a software renderer standing in for a
GPU: each is a variation in its own right, and is recorded as faithfully as a GPU's own arithmetic. Mesa's Intel
Vulkan driver is the worked example: on a UHD Graphics 630 it packs a fast clear's colour on the CPU, correctly
rounded, so the `webgpu-clear` set gives the exact reference answers; with `INTEL_DEBUG=nofc` turning fast clears off,
the same GPU gives what it gives under Metal on macOS. Both are sightings to keep, each with the setting that made it.

So when collecting, run every probe set on every adapter, through every interface, browser and backend the machine
offers -- and again under each driver setting that changes the answer. A sighting that matches an entry already in
the registry still adds its row, because where a signature has been seen is as much the record as the signature.
Never leave out, correct or "fix" a sighting because it looks wrong or unexpected; an unexpected answer is the
registry doing its job. Record it, with everything known about how it was taken, and work out why afterwards.

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

1. **wgpu directly, and the CPU.** `gpu-probe` puts every probe set to each adapter wgpu finds through Vulkan, Metal,
   Direct3D 12 and OpenGL, software renderers included, and the `host` set to the CPU, adding what the registry
   lacks. Each row names the driver and records, as `environment`, every driver setting in force:

   ```bash
   mise exec -- cargo run -p et-gpu-probe-server --bin gpu-probe -- --device "HP EliteBook 645 G10"
   ```

   Then run it again under each driver setting that can change an answer. On Linux, wgpu opens one OpenGL adapter,
   whichever GPU the EGL vendor glvnd loads gives, so with more than one vendor or GPU, reach each in turn:
   `__EGL_VENDOR_LIBRARY_FILENAMES=/usr/share/glvnd/egl_vendor.d/50_mesa.json` for Mesa where NVIDIA's EGL is the
   default, with `DRI_PRIME=pci-0000_00_02_0` (a GPU's PCI address) for each GPU Mesa drives, and
   `LIBGL_ALWAYS_SOFTWARE=1` for llvmpipe. On an Intel GPU under Mesa, add `INTEL_DEBUG=nofc` for its clears without
   fast clears. NVIDIA's EGL gives wgpu an OpenGL 3.3 context, which runs no compute shaders, so there only the
   clears and fixed-function probes are measured, and `gpu-probe` says which sets were not.

2. **Every browser, as WASM.** The probe page runs WebGL 1 and 2 on the GPU the browser picks, WebGPU on each
   adapter it offers (high-performance, low-power and its software fallback), and the canvas fingerprints. On Linux
   and macOS, run each installed browser in turn, and Chrome on Linux on each ANGLE backend:

   ```bash
   mise run gpu-probe-browser firefox --device "Dell Precision 5570" --submit
   mise run gpu-probe-chrome vulkan --device "Dell Precision 5570" --submit
   ```

   Run them again under each driver setting that can change an answer, given to either task as `--environment`, which
   records it in the rows. On Linux with two GPUs, Chrome runs WebGL and WebGPU on the one driving the display, so
   where the other has a Vulkan driver of its own, reach it by naming that driver alone; two GPUs sharing one driver
   cannot be told apart this way. On a hybrid laptop, name the setting that runs the browser on the discrete GPU:

   ```bash
   vulkan="VK_DRIVER_FILES=/usr/share/vulkan/icd.d/nvidia_icd.json"
   mise run gpu-probe-chrome vulkan --device "Dell Precision Tower 5810" --environment "$vulkan" --submit
   ```

   Firefox on Linux offers WebGPU only with `--webgpu`, and masks the adapter's name, so a row it gives through a
   driver setting reaching one GPU alone is filed as that GPU.

   ANGLE's `gl` backend reaches a GPU that NVIDIA's own driver drives, while another drives the display, only with
   `--headless`, through NVIDIA's EGL:

   ```bash
   egl="__EGL_VENDOR_LIBRARY_FILENAMES=/usr/share/glvnd/egl_vendor.d/10_nvidia.json"
   mise run gpu-probe-chrome gl --headless --device "Dell Precision Tower 5810" --environment "$egl" --submit
   ```

   On Windows neither task runs, so serve the page with `mise run gpu-probe-page` and open
   `http://localhost:8099/?submit&device=<machine>` in each browser from a fresh profile (`--user-data-dir` for
   Chrome and Edge, `--no-remote --profile` with `.mise/gpu-probe-browser.user.js` as its `user.js` for Firefox), with
   `&browser=edge&browser_version=<version>` for Edge, which reports Chromium's. Open Chrome and Edge once with each
   `--use-angle=` of `d3d11`, `d3d11on12`, `d3d11-warp`, `gl`, `vulkan` and `swiftshader` (the last with
   `--enable-unsafe-swiftshader`), each of which runs WebGL through another API or renderer, once with
   `--enable-unsafe-webgpu --enable-unsafe-swiftshader`, without which they give WebGPU no fallback adapter, and once
   in a private window (`--incognito`, `--inprivate`, Firefox's `-private-window`, with `&window=private`).
   On a machine with two GPUs, add `--force_high_performance_gpu` for WebGL on the other one. A browser started under
   a driver setting, such as `INTEL_DEBUG=nofc`, names it with `&environment=INTEL_DEBUG%3Dnofc`, since a page cannot
   see it. Ubuntu's snap Chromium sees only Mesa's drivers, so it reaches no GPU that NVIDIA's own driver drives.

3. **`wasi:nn`.** ONNX Runtime behind `et-ws-wasi-runner`, under each execution target; without the `cuda` feature
   every target runs on the CPU. It proposes what it measured as a file in `signatures/submitted/`, named by an
   absolute path, since a test runs in its own package's directory:

   ```bash
   export GPU_PROBE_DEVICE="HP EliteBook 645 G10" GPU_PROBE_INTO="$PWD/libs/gpu-probe/signatures/submitted"
   mise exec -- cargo test -p et-ws-wasi-runner --test nn_probe -- --nocapture
   ```

   On Linux x64 with an NVIDIA GPU, run it again through CUDA. `et-ws-wasi-runner` reaches an NVIDIA GPU through
   `wasi:nn` only when it is built with its `cuda` feature and loads an ONNX Runtime that carries the CUDA execution
   provider. The opt-in `MISE_ENV=cuda` installs that GPU build of ONNX Runtime, which is published for Linux x64
   alone, and points `ORT_DYLIB_PATH` at it. Stack it after the other envs:

   ```bash
   MISE_ENV=rust,cuda mise install
   ```

   The provider also needs the CUDA 12 runtime, cuBLAS, cuFFT, cuRAND, NVRTC and cuDNN 9 on the host, which no mise
   tool supplies. On Ubuntu 24.04 or Pop!_OS 24.04 they come from NVIDIA's own apt repository; install only these
   libraries, since the `cuda` and `cuda-drivers` metapackages would replace the distribution's NVIDIA driver:

   ```bash
   repo=https://developer.download.nvidia.com/compute/cuda/repos/ubuntu2404/x86_64
   curl -fsSLo target/cuda-keyring.deb "$repo/cuda-keyring_1.1-1_all.deb"
   sudo dpkg -i target/cuda-keyring.deb
   sudo apt-get update
   sudo apt-get install -y --no-install-recommends cuda-cudart-12-9 cuda-nvrtc-12-9 libcudnn9-cuda-12
   sudo apt-get install -y --no-install-recommends libcublas-12-9 libcufft-12-9 libcurand-12-9
   echo /usr/local/cuda-12.9/targets/x86_64-linux/lib | sudo tee /etc/ld.so.conf.d/cuda-12-9.conf
   sudo ldconfig
   ```

   A missing library does not fail anything: ONNX Runtime runs the session on the CPU without saying so. Check that
   every library the provider links resolves, which prints nothing when they all do:

   ```bash
   MISE_ENV=cuda mise exec -- sh -c 'ldd "${ORT_DYLIB_PATH%/*}/libonnxruntime_providers_cuda.so" | rg "not found"'
   ```

   The provider also has to carry kernels for the GPU's compute capability, which this prints:

   ```bash
   nvidia-smi --query-gpu=name,compute_cap --format=csv
   ```

   ONNX Runtime 1.22's has none for Maxwell: a Quadro M2000 and a GeForce GTX 950, both at 5.2, fail their first
   kernel with `cudaErrorNoKernelImageForDevice: no kernel image is available for execution on the device`. No
   package or mise tool supplies them, so such a GPU is measured through CUDA only by an ONNX Runtime built with
   `CMAKE_CUDA_ARCHITECTURES` naming it, against CUDA 12, the last to compile for Maxwell; any later GPU, Turing
   onwards, needs nothing more.

   Then run the probe through it:

   ```bash
   MISE_ENV=rust,cuda mise exec -- cargo test -p et-ws-wasi-runner --features cuda --test nn_probe -- --nocapture
   ```

   For a Maxwell GPU, point `ORT_DYLIB_PATH` at the build's own library instead. `MISE_ENV=cuda` sets it to the
   package's, over any value the shell exports, so it is set inside the command mise runs:

   ```bash
   ort=/path/to/onnxruntime/build/Linux/Release/libonnxruntime.so
   test="cargo test -p et-ws-wasi-runner --features cuda --test nn_probe -- --nocapture"
   MISE_ENV=rust,cuda mise exec -- sh -c "ORT_DYLIB_PATH=$ort $test"
   ```

   On a machine with an NVIDIA GPU, the workspace's lints build `et-gpu-probe-server`'s `cuda` feature as well. The
   libraries above are CUDA 12 and carry no headers, so it needs CUDA 13's `cuda.h` and `curand.h` beside them, of
   the CUDA version `nvidia-smi` reports for the driver:

   ```bash
   sudo apt-get install -y --no-install-recommends cuda-cudart-dev-13-0 cuda-crt-13-0 libcurand-dev-13-0
   ```

   They are found under `/usr/local/cuda-13` or `/usr/local/cuda`; where neither links to them, name them with
   `export CUDA_HOME=/usr/local/cuda-13.0` before building. Running what that feature builds needs only the driver,
   which compiles its PTX for the GPU it runs on, so a Maxwell GPU that CUDA 13 no longer compiles for still answers it.

4. **Deno.** The graphics-info module puts every WebGPU probe set to each adapter Deno's WebGPU gives, under
   `et-ws-web-runner`, and proposes them the same way:

   ```bash
   mise run build-ws-graphics-info-module
   export GPU_PROBE_DEVICE="HP EliteBook 645 G10" GPU_PROBE_INTO="$PWD/libs/gpu-probe/signatures/submitted"
   mise exec -- cargo test -p et-ws-web-runner --test modules graphics_info -- --nocapture
   ```

   On Windows, run both under `MISE_ENV=rust,msvc`, after `mise run prefetch:msvc` stages the portable MSVC
   toolchain: the default gnullvm target has no V8 prebuilt, and `MISE_ENV=mingw`'s gnu link of the msvc one stops
   on v8 150.4.0 with `undefined symbol: v8__V8__EnableWebAssemblyTrapHandler`. A clone that has moved since it was
   staged needs the task run again, as `target/msvc/msvc.env` names the toolchain by its absolute path.

   It measures through the operating system's own API, so run it once more for each other backend the machine has,
   naming it by the registry's name: `DENO_WEBGPU_BACKEND=vulkan` on Windows, say. On an integrated GPU short of
   memory, the module's later WebGPU probes may report having no device; the probe sets are measured first, and the
   run still passes.

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

Each GPU adapter's benchmarks go in a file of their own, `benches/results/<machine>-<gpu>-<os>.yaml`, named for the
adapter as wgpu names it and the API it was opened through, such as `quadro-m2000-vulkan`, and recording what it
reports of itself: its driver and their versions, and whether it runs compute shaders. The machine's file keeps the
benchmarks that run on the CPU or open every adapter at once, and lists every adapter the machine gave, with the
reason for any whose device did not open. Run it with every GPU installed, and again under each EGL vendor and
`DRI_PRIME` the collecting steps use, since each makes another GPU wgpu's one OpenGL adapter.

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
