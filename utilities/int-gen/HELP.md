# Command-Line Help for `et-int-gen`

This document contains the help content for the `et-int-gen` command-line program.

**Command Overview:**

- [`et-int-gen`↴](#et-int-gen)
- [`et-int-gen generate`↴](#et-int-gen-generate)
- [`et-int-gen fetch-deps`↴](#et-int-gen-fetch-deps)
- [`et-int-gen checks`↴](#et-int-gen-checks)
- [`et-int-gen help-md`↴](#et-int-gen-help-md)

## `et-int-gen`

Generate checked-in artifacts (generated/, CHECKS.md, HELP.md) from in-repo sources of truth

**Usage:** `et-int-gen [COMMAND]`

###### **Subcommands:**

- `generate` — Emit the generated artifacts for one target (default: all)
- `fetch-deps` — Fetch upstream WASI WIT packages into generated/specs/wit/ at pinned versions
- `checks` — Write CHECKS.md, the catalogue of every check across every `MISE_ENV`
- `help-md` — Write each utility's HELP.md from its clap command tree

## `et-int-gen generate`

Emit the generated artifacts for one target (default: all)

**Usage:** `et-int-gen generate [TARGET]`

###### **Arguments:**

- `<TARGET>` — Which artifacts to emit; defaults to `all`

  Default value: `all`

  Possible values:
  - `core`:
    Language-agnostic specs: AsyncAPI/OpenAPI YAML, WIT, KDL, schema JSON
  - `rust`:
    The typed Rust REST client
  - `bindings`:
    The wasmtime host bindings for the ws-wasi-runner `runner` world
  - `zig`:
    The Zig REST client (skipped when openapi2zig is absent)
  - `all`:
    Core + Rust + bindings + Zig

## `et-int-gen fetch-deps`

Fetch upstream WASI WIT packages into generated/specs/wit/ at pinned versions

**Usage:** `et-int-gen fetch-deps`

## `et-int-gen checks`

Write CHECKS.md, the catalogue of every check across every `MISE_ENV`

**Usage:** `et-int-gen checks [OPTIONS]`

###### **Options:**

- `--check` — Compare against the committed CHECKS.md and fail on drift instead of writing it

## `et-int-gen help-md`

Write each utility's HELP.md from its clap command tree

**Usage:** `et-int-gen help-md [OPTIONS]`

###### **Options:**

- `--check` — Compare against the committed HELP.md files and fail on drift instead of writing them

<hr/>

<small><i>
This document was generated automatically by
<a href="https://crates.io/crates/clap-markdown"><code>clap-markdown</code></a>.
</i></small>
