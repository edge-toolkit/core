# et-gpu-probe

Numeric and fixed-function probes that tell one GPU's arithmetic from another's, the registry of every signature they
have been seen to give, and, with the `native` feature, the driver that puts them to a GPU through wgpu.

## Benchmarks

`benches/gpu_probe.rs` measures every operation the crate offers: building each probe set and its exact answers, the
texture formats' conversions, reading a signature through each interface, the registry, and opening the GPUs and
putting every probe set to each adapter the machine has. Record a machine's results with

```bash
mise run bench-gpu-probe dell-precision-5570
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
