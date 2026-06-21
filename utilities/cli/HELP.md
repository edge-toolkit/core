# Command-Line Help for `et-cli`

This document contains the help content for the `et-cli` command-line program.

**Command Overview:**

- [`et-cli`↴](#et-cli)
- [`et-cli generate-deployment`↴](#et-cli-generate-deployment)
- [`et-cli regen-verification`↴](#et-cli-regen-verification)
- [`et-cli module-package-json`↴](#et-cli-module-package-json)

## `et-cli`

**Usage:** `et-cli [COMMAND]`

###### **Subcommands:**

- `generate-deployment` — Generate deployment config from a cluster input YAML
- `regen-verification` — Regenerate verification outputs using verification input/output naming conventions
- `module-package-json` — Generate pkg/package.json from module metadata

## `et-cli generate-deployment`

Generate deployment config from a cluster input YAML

**Usage:** `et-cli generate-deployment [OPTIONS] --input-file <INPUT_FILE> --output-dir <OUTPUT_DIR>`

###### **Options:**

- `--input-file <INPUT_FILE>`
- `--output-dir <OUTPUT_DIR>`
- `--output-type <OUTPUT_TYPE>`

  Default value: `mise`

  Possible values: `mise`, `docker-compose`

## `et-cli regen-verification`

Regenerate verification outputs using verification input/output naming conventions

**Usage:** `et-cli regen-verification [OPTIONS]`

###### **Options:**

- `--verification-root <VERIFICATION_ROOT>`

  Default value: `verification`

## `et-cli module-package-json`

Generate pkg/package.json from module metadata

**Usage:** `et-cli module-package-json [OPTIONS]`

###### **Options:**

- `--module-dir <MODULE_DIR>`

  Default value: `.`

<hr/>

<small><i>
This document was generated automatically by
<a href="https://crates.io/crates/clap-markdown"><code>clap-markdown</code></a>.
</i></small>
