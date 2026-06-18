# Command-Line Help for `et-int-gen`

This document contains the help content for the `et-int-gen` command-line program.

**Command Overview:**

* [`et-int-gen`↴](#et-int-gen)
* [`et-int-gen generate`↴](#et-int-gen-generate)
* [`et-int-gen fetch-deps`↴](#et-int-gen-fetch-deps)

## `et-int-gen`

Generate checked-in artifacts under generated/ from in-repo Rust sources of truth

**Usage:** `et-int-gen [COMMAND]`

###### **Subcommands:**

* `generate` — Emit the generated artifacts for one target (default: all)
* `fetch-deps` — Fetch upstream WASI WIT packages into generated/specs/wit/ at pinned versions



## `et-int-gen generate`

Emit the generated artifacts for one target (default: all)

**Usage:** `et-int-gen generate [TARGET]`

###### **Arguments:**

* `<TARGET>` — Which artifacts to emit; defaults to `all`

  Default value: `all`

  Possible values:
  - `core`:
    Language-agnostic specs: AsyncAPI/OpenAPI YAML, WIT, KDL, schema JSON
  - `rust`:
    The typed Rust REST client
  - `zig`:
    The Zig REST client (skipped when openapi2zig is absent)
  - `all`:
    Core + Rust + Zig




## `et-int-gen fetch-deps`

Fetch upstream WASI WIT packages into generated/specs/wit/ at pinned versions

**Usage:** `et-int-gen fetch-deps`



<hr/>

<small><i>
    This document was generated automatically by
    <a href="https://crates.io/crates/clap-markdown"><code>clap-markdown</code></a>.
</i></small>

