# Taplo schema rules — backlog and notes

Companion to the schemas under `config/taplo/` and their wiring in
`.taplo.toml`. Captures rule ideas we discussed but haven't implemented, plus
one significant caveat about how rule-scoped schemas are enforced.

## Active rules

- **`no-path-deps.schema.json`** — every member `Cargo.toml` (all dep tables).
  Catches `foo = { path = "..." }`.
- **`require-lints-section.schema.json`** — every member `Cargo.toml` except
  the workspace root and `generated/rust-rest/Cargo.toml`. Catches missing
  `[lints] workspace = true`.
- **`no-wildcard-or-git-deps.schema.json`** — member `Cargo.toml`,
  `[dependencies]` and `[dev-dependencies]`. Catches `foo = "*"` and
  inline `foo = { git = "..." }`.
- **`require-workspace-deps.schema.json`** — every member `Cargo.toml`,
  `[dependencies]` and `[dev-dependencies]` (and the `[target.*]`
  variants). Catches anything that isn't `foo.workspace = true`, including
  string-version deps and inline tables that omit `workspace = true`.
  `[build-dependencies]` isn't covered yet.
- **`require-lib-doctest-false.schema.json`** — every member `Cargo.toml`
  that declares `[lib]`. Requires `doctest = false` so cargo's doctest
  harness doesn't try to compile prose code-fences as Rust. Pairs with
  the `no-doctest` ast-grep rule (which bans `` ``` `` blocks inside
  `///` / `//!` comments). Runnable examples go in `tests/` files.
- **`no-anyhow-dep.schema.json`** — every `Cargo.toml` (including the
  workspace root). Forbids `anyhow` as a dep in `[dependencies]`,
  `[dev-dependencies]`, `[build-dependencies]`, `[workspace.dependencies]`,
  and the `[target.*]` variants. Replaces the old `no-anyhow` ast-grep
  rule (now removed) — that one caught _uses_, this one stops the
  declaration one stage earlier. Use a `thiserror` enum.

## How taplo's rule-scoped schemas are enforced

`taplo lint` (the command `mise run taplo-check` invokes) honours simple
top-level constraints from `[[rule]] schema` configs in `.taplo.toml` (e.g.
`required: [...]` — that's why `require-lints-section` fires when invoked
through the rule mechanism), but it silently skips nested constraints
(`patternProperties`, `additionalProperties`, `$ref`-driven sub-schemas)
in CLI mode. `no-path-deps`, `no-wildcard-or-git-deps`, and
`require-workspace-deps` all fall into the second bucket: they validate
correctly only when invoked explicitly via `taplo lint --schema
file://...`.

To get them enforced under `mise run taplo-check`, the task explicitly
applies each schema via `--schema` per-file. See the `[tasks.taplo-check]`
block in `.mise.toml`. The `.taplo.toml` `[[rule]]` entries remain so
editor / LSP integrations still pick the schemas up for inline
validation; CI enforcement is the task body.

When adding a new schema under `config/taplo/`:

1. Drop it next to the existing schemas.
2. Add a `[[rule]]` entry in `.taplo.toml` so editors pick it up.
3. Add a `taplo lint --schema "file://$PWD/config/taplo/X.schema.json"
   "${members[@]}"` line in `[tasks.taplo-check]` so CI enforces it.

## Backlog — other rule ideas

Ordered by perceived value for this repo.

### 1. Require workspace-inherited metadata on member crates

Most crates already do `edition.workspace = true`, `license.workspace =
true`, `repository.workspace = true`. A schema can enforce this so a new
crate doesn't drift onto a hardcoded `edition = "2021"` and miss a future
workspace bump. Same `const`-as-error-message trick as `no-path-deps`:

```json
{
  "if": { "required": ["package"] },
  "then": {
    "properties": {
      "package": {
        "properties": {
          "edition": { "$ref": "#/definitions/mustBeWorkspace" },
          "license": { "$ref": "#/definitions/mustBeWorkspace" },
          "repository": { "$ref": "#/definitions/mustBeWorkspace" }
        }
      }
    }
  }
}
```

with `mustBeWorkspace` matching only `{ workspace = true }`. Subject to the
same `taplo lint` caveat above.

### 2. Forbid stray feature flags

Cargo lets you declare a feature with the same name as a dep, and lets
features be empty arrays. Both are code smells. A schema can require
feature values be non-empty arrays and forbid features sharing a dep name.

### 3. Require `description` and `run` on every `[tasks.<name>]`

`mise tasks ls` shows the `description`. Several existing tasks don't have
one (e.g. `taplo-fmt`, `taplo-check`, `typos`, the `ruff-*` keys). Schema
makes every new task self-documenting. Requires the same workaround if
nested constraints are needed, but `required` at the top level should fire
via plain `taplo lint`.

### 4. Forbid `path = "..."` inside `[tool.uv.sources]` for `pyproject.toml`

Parallel of `no-path-deps` for Python's `uv` tool config. Same pattern,
different file shape (and a different glob in `.taplo.toml`).

### 5. Forbid other legacy error crates (`failure`, `error-chain`)

The `no-anyhow-dep` schema covers `anyhow`. Extending the same `const`-as-
error trick to `failure` and `error-chain` would be a one-line addition
per crate. Low priority — neither is in the current dep graph and both
are largely unmaintained, so a future PR would have to introduce them
intentionally.

### 6. Require crate `name` prefix `et-` under `services/`/`libs/`

Catches new crates that drift from the naming convention. Minor.

### 7. Pin task `dir = "..."` to known paths

Probably overkill — a typo in `dir` is caught by the first task run anyway.
