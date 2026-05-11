# et-cli

`et-cli` is the Edge Toolkit management CLI.

## Enable Direct `et-cli` Usage

From the repository root, install or refresh the local CLI with:

```bash
cargo install --path utilities/cli --force
```

If `et-cli` is not found after installation, make sure `~/.cargo/bin` is on your `PATH`.

Confirm the installed CLI is available:

```bash
et-cli generate-deployment --help
et-cli regen-verification --help
```

## Deployment Modes

Deployment generation supports two runtime modes:

- `local` is the default. Use it when running from the `core` source checkout.
  It resolves the
  repository root and generates deployment files that use the source-tree
  layout.
- `published` is for running `et-cli` outside the `core` source checkout. In
  this mode, pass `--edge-toolkit-path` to the installed Edge Toolkit runtime
  bundle root. The generated `mise.toml` runs
  `<edge-toolkit-path>/target/release/et-ws-server` instead of `cargo run`.

If `--mode published` is used without `--edge-toolkit-path`, the path still
defaults to `../../libs/edge-toolkit`.

For example:

```bash
et-cli generate-deployment \
  --mode published \
  --edge-toolkit-path <edge-toolkit-path> \
  --input-file ./scenario.yaml \
  --output-dir ./output \
  --output-type mise
```

## Generate a Mise Deployment

Run `generate-deployment` with `--output-type mise` to generate a
`mise.toml` file in local mode, specifying the input YAML file and output
directory as follows:

```bash
et-cli generate-deployment \
  --input-file verification/local/input/<some-scenario>.yaml \
  --output-dir verification/local/output/<some-scenario> \
  --output-type mise
```

Then, to run the deployment:

```bash
mise run generated-scenario
```

The generated `ws-server` tasks set `MODULES_PATHS` so the server only exposes
the scenario's selected workflow modules plus `ws-wasm-agent`, using the
configurable module-path logic in `ws-server`. In local mode, the generated
`mise.toml` runs `cargo run` from the repository `services/ws-server` directory.
In published mode, it runs `target/release/et-ws-server` from
`--edge-toolkit-path`. It does not build modules; it assumes builds are handled
externally.

## Generate a Docker Compose Deployment

Run `generate-deployment` with `--output-type docker-compose` to generate a
`compose.yaml` file:

```bash
et-cli generate-deployment \
  --input-file verification/local/input/<some-scenario>.yaml \
  --output-dir verification/local/output/<some-scenario> \
  --output-type docker-compose
```

Then, to run the deployment from the output directory:

```bash
docker compose up --build
```

In local mode, the generated compose stack starts OpenObserve and builds
`ws-server` from the repository Dockerfile. Native build dependencies such as
`protoc` are installed in the image build stage, so the runtime container does
not depend on host Rust, Cargo caches, or `mise` tools. In published mode, the
generated service bind-mounts
`<edge-toolkit-path>/target/release/et-ws-server` to
`/usr/local/bin/et-ws-server` in the container. The generated service sets
`MODULES_PATHS`, OpenTelemetry auth, and the in-compose OpenObserve collector
URL for the selected scenario.

## Regenerate Verification Outputs

Run `regen-verification` to regenerate checked-in verification outputs from the
verification root. By default it runs in local mode, reads scenario files under
`verification/local/input`, and writes every supported deployment type to the
matching `verification/local/output/<input-file-stem>` folder.

Currently this writes both `mise.toml` and `compose.yaml`. Future deployment
types should be added to the shared supported output type list so
`regen-verification` picks them up automatically.

```bash
et-cli regen-verification
```

Use published mode to regenerate only `verification/published`. Published
verification regeneration requires `--edge-toolkit-path`:

```bash
et-cli regen-verification \
  --mode published \
  --edge-toolkit-path <edge-toolkit-path>
```

For example, `verification/local/input/facility-security-scenario.yaml`
regenerates local deployment outputs into
`verification/local/output/facility-security-scenario`.

## Input YAML

The input YAML must include a `cluster_name` and an `agents` list. Each agent
must include `name` and `resources`.

```yaml
cluster_name: example
agents:
  - name: camera-agent
    resources:
      - type: face-detection
```

Required fields:

- `cluster_name`: scenario name used in generated task descriptions.
- `agents[].name`: agent template name.
- `agents[].resources`: list of device-backed resources for the agent.
- `agents[].resources[].type`: workflow module name as a plain string. This is
  resolved dynamically and is not limited to a fixed enum in `et-cli`.

Optional fields:

- `deployment_type`: `mise` or `docker-compose`; defaults to `mise` when
  omitted.
- `deployment_mode`: `local` or `published`; used by
  `regen-verification` as a consistency check against the selected verification
  set.

The generated deployment config uses `agents[].resources[].type` values to
decide which module directories to expose through `MODULES_PATHS`. It only
serves `ws-wasm-agent` and the selected workflow modules for that scenario,
without adding any module build tasks.

#### Notes on deployment_type

The input YAML can also choose the output type with `deployment_type`.

```yaml
cluster_name: example
deployment_type: docker-compose
agents:
  - name: camera
    resources:
      - type: face-detection
```

If both are present, the command-line `--output-type` value wins over
`deployment_type` in the input file.

#### Notes on deployment_mode

Verification scenario inputs can choose the generation mode with
`deployment_mode`.

```yaml
cluster_name: example
deployment_mode: published
agents:
  - name: camera
    resources:
      - type: face-detection
```

The `deployment_mode` field is used as a consistency check for
`regen-verification`: scenarios under `verification/local/input` should say
`deployment_mode: local`, and scenarios under `verification/published/input`
should say `deployment_mode: published`. Run `regen-verification --mode
published --edge-toolkit-path <path>` to regenerate published scenarios.

## Use Without Installing

You can also run the CLI through Cargo from the repository root:

```bash
cargo run -p et-cli -- generate-deployment \
  --input-file verification/local/input/<some-file>.yaml \
  --output-dir verification/local/output/<some-folder> \
  --output-type docker-compose
```

To regenerate all convention-defined verification outputs through Cargo:

```bash
cargo run -p et-cli -- regen-verification
```
