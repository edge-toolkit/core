# et-cli

`et-cli` is the Edge Toolkit management CLI.

## Enable Direct `et-cli` Usage

From a fresh environment, first make sure Rust and Cargo are installed and that
you are in the repository root.

Build and test the CLI:

```bash
cargo test -p et-cli
```

Install the local CLI binary into Cargo's bin directory:

```bash
cargo install --path utilities/cli
```

Cargo usually installs binaries into `~/.cargo/bin`. Make sure that directory is
on your `PATH`:

```bash
command -v et-cli
```

If the command prints nothing, add Cargo's bin directory to your shell profile.
For Bash:

```bash
echo 'export PATH="$HOME/.cargo/bin:$PATH"' >> ~/.bashrc
source ~/.bashrc
```

For Zsh:

```bash
echo 'export PATH="$HOME/.cargo/bin:$PATH"' >> ~/.zshrc
source ~/.zshrc
```

Confirm the installed CLI is available:

```bash
et-cli generate-deployment --help
```

## Quickstart

From the repository root, generate the default facility security scenario:

```bash
et-cli generate-deployment
```

Go to the generated output directory:

```bash
cd verification/local/output/facility-security-example
```

Run the generated scenario:

```bash
mise run generated-scenario
```

That builds `ws-wasm-agent`, builds only the workflow modules referenced by the
input YAML, and starts the WebSocket server.

## Generate a Mise Deployment

For a custom input scenario, run the following command, where `<some-scenario>`
is your scenario name:

```bash
et-cli generate-deployment \
  --input-file verification/local/input/<some-scenario>.yaml \
  --output-dir verification/local/output/<some-scenario> \
  --output-type mise
```

From the output directory, run:

```bash
mise run build-wasm
mise run ws-server
```

`build-wasm` builds `ws-wasm-agent` and only the workflow modules referenced by
the input YAML. `ws-server` runs the workspace WebSocket server.

The generated `.mise.toml` also includes a convenience task that runs `build-wasm` and then `ws-server`.

```bash
mise run generated-scenario
```

## Default Scenario

When no paths or output type are provided:

```bash
et-cli generate-deployment
```

This is equivalent to:

```bash
et-cli generate-deployment \
  --input-file verification/local/input/facility-security-example.yaml \
  --output-dir verification/local/output/facility-security-example/ \
  --output-type mise
```

## Input YAML

The input YAML must include a `cluster_name` and an `agents` list. Each agent
must include `name`, `capabilities`, and `modules`.

```yaml
cluster_name: example
deployment_type: mise
agents:
  - name: camera-agent
    count: 1
    capabilities: [camera]
    modules:
      - name: face-detection
        config:
          confidence_threshold: 0.85
```

Required fields:

- `cluster_name`: scenario name used in generated task descriptions.
- `agents[].name`: agent template name.
- `agents[].capabilities`: list of capabilities; use `[]` if none are needed.
- `agents[].modules`: list of workflow modules for the agent.
- `agents[].modules[].name`: module directory name under `services/ws-modules/`.

Optional fields:

- `deployment_type`: currently only `mise`; defaults to `mise` when omitted.
- `agents[].count`: defaults to `1` when omitted.
- `agents[].modules[].config`: parsed for future use, but not used by the
  generated `.mise.toml` today.

The generated `.mise.toml` uses module names to decide which
`services/ws-modules/<module>` tasks to build, and always includes
`build-ws-wasm-agent`, `ws-server`, and `generated-scenario`.

#### Notes on deployment_type

The input YAML can also choose the output type with `deployment_type`. `mise` is
the only supported value for now; the field and CLI option are present so more
deployment types can be added later.

```yaml
cluster_name: example
deployment_type: mise
agents:
  - name: camera
    capabilities: [camera]
    modules:
      - name: face-detection
```

If both are present, the command-line `--output-type` value wins over
`deployment_type` in the input file.

## Use Without Installing

You can also run the CLI through Cargo from the repository root:

```bash
cargo run -p et-cli -- generate-deployment \
  --input-file verification/local/input/<some-file>.yaml \
  --output-dir verification/local/output/<some-folder> \
  --output-type mise
```
