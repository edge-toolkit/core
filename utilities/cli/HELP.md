# Command-Line Help for `et-cli`

This document contains the help content for the `et-cli` command-line program.

**Command Overview:**

- [`et-cli`](#et-cli)
- [`et-cli generate-deployment`](#et-cli-generate-deployment)
- [`et-cli regen-verification`](#et-cli-regen-verification)
- [`et-cli module-package-json`](#et-cli-module-package-json)
- [`et-cli block-ip`](#et-cli-block-ip)
- [`et-cli evict-agent`](#et-cli-evict-agent)
- [`et-cli npm-module-path`](#et-cli-npm-module-path)

## `et-cli`

**Usage:** `et-cli [COMMAND]`

### Subcommands

- `generate-deployment` -- Generate deployment config from a cluster input YAML
- `regen-verification` -- Regenerate verification outputs using verification input/output naming conventions
- `module-package-json` -- Generate pkg/package.json from module metadata
- `block-ip` -- Block an IP address on a running hub and every hub sharing its backplane
- `evict-agent` -- Evict an agent from a running hub, blocking the IP address it last connected from on every hub
- `npm-module-path` -- Print the directory holding a mise-staged npm package

## `et-cli generate-deployment`

Generate deployment config from a cluster input YAML

**Usage:** `et-cli generate-deployment [OPTIONS] --input-file <INPUT_FILE> --output-dir <OUTPUT_DIR>`

### Options

- `--input-file <INPUT_FILE>`
- `--output-dir <OUTPUT_DIR>`
- `--output-type <OUTPUT_TYPE>`

  Default value: `mise`

  Possible values: `mise`, `dekit`, `process-compose`, `docker-compose`, `k3s`

## `et-cli regen-verification`

Regenerate verification outputs using verification input/output naming conventions

**Usage:** `et-cli regen-verification [OPTIONS]`

### Options

- `--verification-root <VERIFICATION_ROOT>`

  Default value: `verification`

## `et-cli module-package-json`

Generate pkg/package.json from module metadata

**Usage:** `et-cli module-package-json [OPTIONS]`

### Options

- `--module-dir <MODULE_DIR>`

  Default value: `.`

## `et-cli block-ip`

Block an IP address on a running hub and every hub sharing its backplane

**Usage:** `et-cli block-ip --hub-url <HUB_URL> --ip <IP> --admin-token <ADMIN_TOKEN>`

### Options

- `--hub-url <HUB_URL>` -- Base URL of the hub, e.g. `http://localhost:8080`
- `--ip <IP>` -- IPv4 or IPv6 address to block
- `--admin-token <ADMIN_TOKEN>` -- The hub's admin bearer token; set `WS_ADMIN_TOKEN` to keep it out of the process list

## `et-cli evict-agent`

Evict an agent from a running hub, blocking the IP address it last connected from on every hub

**Usage:** `et-cli evict-agent --hub-url <HUB_URL> --agent-id <AGENT_ID> --admin-token <ADMIN_TOKEN>`

### Options

- `--hub-url <HUB_URL>` -- Base URL of the hub, e.g. `http://localhost:8080`
- `--agent-id <AGENT_ID>` -- Id of the agent to evict
- `--admin-token <ADMIN_TOKEN>` -- The hub's admin bearer token; set `WS_ADMIN_TOKEN` to keep it out of the process list

## `et-cli npm-module-path`

Print the directory holding a mise-staged npm package

**Usage:** `et-cli npm-module-path --package <PACKAGE>`

### Options

- `--package <PACKAGE>` -- Published package name, as it appears in the mise tool id (e.g. `onnxruntime-web`)
