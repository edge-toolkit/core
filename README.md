# edge-toolkit core

## mise

Please install [`mise`](https://mise.jdx.dev/).
It is needed for all use of this repository.

Configure it with

```bash
mise settings experimental=true
mise settings set cargo.binstall true
```

## Contributing

Use `mise run fmt` and `mise run check` to run formatters and checkers.

## Run e2e

Run the end-to-end tests using Chrome:

```bash
mise run ws-e2e-chrome
```

## Run ws agent in browser

### Build modules and run the WS server

In a separate terminal start OpenObserve (o2) and leave it running.

```bash
mise run o2
```

Then start the fetch the ONNX models and run the server

```bash
mise run download-models
mise run build-modules
mise run ws-server
```

Scan the QR-Code with a smart-phone camera and open the URL.

Select the module to run in the drop-down, then click "Run module" button.

Note: The WASM build disables WebAssembly reference types, so it can still load on older browsers such as Chrome 95.

In a separate terminal, open the OpenObserve UX using:

```bash
mise run open-o2
```

The server logs appear in the Logs section.

## Modules

The module list is dynamically populated from the modules in [services/ws-modules](services/ws-modules).

Each module must be a directory `pkg` containing a `package.json` that defines a `main` which contains a JavaScript file
that can load and run the module.

Most of the module are built from Rust using `wasm-pack build --target web`.

The module `pydata1` uses [pyodide](https://pyodide.org/) to run a Python script.

## Run an example demo scenario using et-cli

```bash
cargo install --path utilities/cli --force
et-cli generate-deployment \
  --input-file verification/local/input/facility-security-scenario.yaml \
  --output-dir verification/local/output/facility-security-scenario
```

This will generate a `mise.toml` file under
`verification/local/output/facility-security-scenario`. Run the following
command to start the demo scenario:

```bash
mise run generated-scenario
```

The generated scenario config only selects which prebuilt modules `ws-server`
serves. Module builds are expected to be handled separately from the repository
root.

To regenerate all checked-in verification outputs from
`verification/*/input`, writing each scenario to
the matching `verification/*/output/<input-file-stem>` folder:

```bash
et-cli regen-verification
```

## Grant

This repository is part of a grant managed by the School of EECMS, Curtin University.

```text
ABN 99 143 842 569.

CRICOS Provider Code 00301J.

TEQSA PRV12158
```
