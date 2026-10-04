# Cargo.toml policy, evaluated over conftest's `--combine` input (an array of {path, contents}).
# Run with `--namespace cargo` (or `--all-namespaces`). Paths come from `git ls-files`, so the workspace root is
# exactly "Cargo.toml" and any other match is a member crate.
package cargo

is_member(file) if {
	endswith(file.path, "Cargo.toml")
	file.path != "Cargo.toml"
}

# Every dependency spec across the dep tables, as [path, name, spec] triples.
dep contains [file.path, name, spec] if {
	some file in input
	endswith(file.path, "Cargo.toml")
	some table in {"dependencies", "dev-dependencies", "build-dependencies"}
	some name, spec in file.contents[table]
}

dep contains [file.path, name, spec] if {
	some file in input
	endswith(file.path, "Cargo.toml")
	some name, spec in file.contents.workspace.dependencies
}

dep contains [file.path, name, spec] if {
	some file in input
	endswith(file.path, "Cargo.toml")
	some tgt in file.contents.target
	some table in {"dependencies", "dev-dependencies", "build-dependencies"}
	some name, spec in tgt[table]
}

# Banned crates -> rejection reason.
# Members must use workspace = true, so the root's [workspace.dependencies] is the only place a ban can bite.
#
# `anyhow` is not listed here, and is instead constrained at the source level: a crate may depend on it, but
# may only name it in an `error.rs` `#[from]` variant.
# A blanket dependency ban was unworkable because `?` on a foreign `anyhow::Result` needs
# `From<anyhow::Error>`, which cannot be written without naming the type.
banned := {
	"openssl": "use rustls + aws-lc-rs -- one TLS/crypto stack only",
	"openssl-sys": "use rustls + aws-lc-rs -- one TLS/crypto stack only",
	"ring": "use aws-lc-rs (transitive via rcgen only; gated in config/deny.toml)",
	"ureq": "use reqwest::blocking or reqwest -- one HTTPS stack only",
}

# The root Cargo.toml must not depend on a crate this repo has banned.
deny contains msg if {
	some [path, name, _] in dep
	path == "Cargo.toml"
	reason := banned[name]
	msg := $"{path}: banned dependency \"{name}\" -- {reason}"
}

# Member crates: no path deps, no wildcard versions, no inline git deps.
# Pins live in the root [workspace.dependencies]; members reference them via workspace = true.
deny contains msg if {
	some [path, name, spec] in dep
	path != "Cargo.toml"
	is_object(spec)
	spec.path
	msg := $"{path}: dependency \"{name}\" uses a path dep; use workspace = true instead"
}

# A member crate must not take a dependency at a wildcard version.
deny contains msg if {
	some [path, name, spec] in dep
	path != "Cargo.toml"
	wildcard(spec)
	msg := $"{path}: dependency \"{name}\" uses a wildcard version; pin via [workspace.dependencies]"
}

# A member crate must not take an inline git dependency.
deny contains msg if {
	some [path, name, spec] in dep
	path != "Cargo.toml"
	is_object(spec)
	spec.git
	msg := $"{path}: dependency \"{name}\" is an inline git dep; pin via [workspace.dependencies]"
}

wildcard(spec) if {
	is_string(spec)
	contains(spec, "*")
}

wildcard(spec) if {
	is_object(spec)
	contains(spec.version, "*")
}

# Member [dependencies]/[dev-dependencies] must inherit via workspace = true so pins stay in [workspace.dependencies].
# (build-dependencies not covered yet.)
deny contains msg if {
	some file in input
	is_member(file)
	some table in {"dependencies", "dev-dependencies"}
	some name, spec in file.contents[table]
	not spec.workspace == true
	msg := sprintf("%s: dependency %q must reference [workspace.dependencies] via workspace = true", [file.path, name])
}

# A crate with a [lib] must disable the doctest harness and must not rename the lib.
# Runnable examples belong in tests/ files; keep the lib name as the package name.
deny contains msg if {
	some file in input
	is_member(file)
	file.contents.lib
	not file.contents.lib.doctest == false
	msg := sprintf("%s: [lib] must set doctest = false", [file.path])
}

# A member crate's [lib] must not rename the library away from the package name.
deny contains msg if {
	some file in input
	is_member(file)
	file.contents.lib.name
	msg := sprintf("%s: [lib] must not set name (keep it the package name)", [file.path])
}

# Every member must inherit the workspace lint tables.
# generated/rust-rest is exempt: progenitor's emitted source trips lints the workspace table denies.
deny contains msg if {
	some file in input
	is_member(file)
	file.path != "generated/rust-rest/Cargo.toml"
	not file.contents.lints.workspace == true
	msg := sprintf("%s: add [lints] workspace = true", [file.path])
}

# Every crate must be a registered workspace member, with no orphan crates.
# Its directory must appear in the root manifest's explicit [workspace].members list.
workspace_member contains m if {
	some file in input
	file.path == "Cargo.toml"
	some m in file.contents.workspace.members
}

# Every member crate must be registered in the root [workspace].members.
deny contains msg if {
	some file in input
	is_member(file)
	dir := trim_suffix(file.path, "/Cargo.toml")
	not workspace_member[dir]
	msg := sprintf("%s: crate is not registered in the root [workspace].members", [file.path])
}

# Shared [package] metadata must be inherited from [workspace.package] via `<field>.workspace = true`.
# This keeps the values defined in exactly one place.
inherited_package_field := {"edition", "license", "repository"}

deny contains msg if {
	some file in input
	is_member(file)
	some field in inherited_package_field
	not file.contents.package[field].workspace == true
	msg := sprintf("%s: [package] %s must inherit via %s.workspace = true", [file.path, field, field])
}

# Crate names are namespaced: "edge-toolkit" or "et-" for normal crates, "int-" marking an internal one.
allowed_crate_name(name) if startswith(name, "edge-toolkit")

allowed_crate_name(name) if startswith(name, "et-")

allowed_crate_name(name) if startswith(name, "int-")

deny contains msg if {
	some file in input
	is_member(file)
	name := file.contents.package.name
	not allowed_crate_name(name)
	msg := sprintf("%s: crate name %q must start with edge-toolkit/et-/int-", [file.path, name])
}

# `publish` is stated in every manifest rather than left to cargo's implicit default.
# Publishing is the consequential choice here (a crates.io upload cannot be withdrawn), so each crate declares
# its intent where a reader looks for it instead of the reader having to know the default.
deny contains msg if {
	some file in input
	is_member(file)
	not is_boolean(file.contents.package.publish)
	msg := sprintf("%s: [package] must set publish explicitly (true, or false for an int- crate)", [file.path])
}

# The "int-" marker and publishability are the same fact, so the name and the flag must agree both ways.
# The marker is anywhere in the name, not just the prefix: `et-int-gen` is as internal as `int-wasm-cov-wrapper`.
# Keeping it biconditional means the crate list cannot drift into a state where a reader has to open the
# manifest to learn whether a crate ships -- the name alone answers it.
deny contains msg if {
	some file in input
	is_member(file)
	name := file.contents.package.name
	contains(name, "int-")
	not file.contents.package.publish == false
	msg := sprintf("%s: crate %q carries the int- marker, so it must set publish = false", [file.path, name])
}

# A crate without the int- marker must set publish = true.
deny contains msg if {
	some file in input
	is_member(file)
	name := file.contents.package.name
	not contains(name, "int-")
	not file.contents.package.publish == true
	msg := sprintf("%s: crate %q must set publish = true; add an int- marker to keep it internal", [file.path, name])
}

# The version each member crate declares, keyed by crate name.
member_version[name] := version if {
	some file in input
	is_member(file)
	name := file.contents.package.name
	version := file.contents.package.version
}

# A path dependency's `version` must match the version its crate actually declares.
# Cargo resolves path deps by path for local builds and only enforces the version requirement when packaging,
# so a stale pin here stays invisible to check/test/clippy and first fails during `cargo publish` -- partway
# through a workspace release, once earlier crates are already uploaded and cannot be withdrawn.
deny contains msg if {
	some file in input
	file.path == "Cargo.toml"
	some name, spec in file.contents.workspace.dependencies
	is_object(spec)
	spec.path
	spec.version != member_version[name]
	msg := sprintf("Cargo.toml: %q pins version %q but that crate declares %q", [name, spec.version, member_version[name]])
}

# Every requirement in the root [workspace.dependencies] names a major and a minor, and stops there.
# A third component is a claim that one specific release is required. Left on by default it claims nothing --
# it is only whatever happened to be current the day the dep was added -- and once most entries carry one, a
# reader can no longer tell the load-bearing pins from the incidental ones, while every routine bump has to
# rewrite a digit that never meant anything. A dep that genuinely needs the patch component says so below,
# in a line that has to name the release and the reason it cannot move.
#
# Path deps are exempt by construction rather than by entry: cargo wants their `version` to equal the member
# crate's own `package.version` -- the rule above enforces exactly that -- which is always a full triple.
patch_pin_exception := {
	"deno_error": "exact pin: JsErrorBox must be the type deno_core holds, so a 0.7.2 would be a second copy",
	"minicov": "exact pin: wasm-bindgen-test 0.3.78 requires =0.3.8, and cargo holds one 0.3.x copy for both",
	"ort": "exact pin on a prerelease, which has no two-part form; rc.11+ moved API wasmtime-wasi-nn calls",
	"wasmtime": "47.0.3 is the security floor; RUSTSEC-2026-0222 has no fix anywhere below it in the 47 line",
	"wasmtime-internal-wit-bindgen": "47.0.4 tracks the wasmtime release; the crate is internal and its API can move",
	"wasmtime-wasi": "47.0.3 carries the same RUSTSEC-2026-0222 floor as the wasmtime entry it ships beside",
}

# Matches the requirement's leading version token rather than splitting the whole string on dots.
# That keeps a comparator (`=`, `>=`, `~`) and a prerelease suffix (`-rc.10`, whose dot is its own) out of the
# component count, so only the version core decides.
patch_pinned(req) if regex.match(`^[^0-9]*[0-9]+\.[0-9]+\.[0-9]+`, req)

requirement(spec) := spec if is_string(spec)

requirement(spec) := spec.version if is_object(spec)

path_dep(spec) if {
	is_object(spec)
	spec.path
}

root_dep[name] := spec if {
	some file in input
	file.path == "Cargo.toml"
	some name, spec in file.contents.workspace.dependencies
}

# A root workspace dependency must be pinned to major.minor, not a patch version, unless it is excepted.
deny contains msg if {
	some name, spec in root_dep
	not path_dep(spec)
	not patch_pin_exception[name]
	patch_pinned(requirement(spec))
	msg := sprintf(
		"Cargo.toml: %q pins %q to a patch version; use major.minor, or add a reasoned patch_pin_exception",
		[name, requirement(spec)],
	)
}

# The exception map is a two-way contract.
# An entry that no longer describes the manifest misleads exactly as much as a missing one would, so a dep that
# has since been trimmed back to major.minor, or dropped altogether, takes its exception entry with it.
exception_is_live(name) if patch_pinned(requirement(root_dep[name]))

deny contains msg if {
	some name, reason in patch_pin_exception
	not exception_is_live(name)
	msg := $"Cargo.toml: patch_pin_exception entry \"{name}\" is stale ({reason}); remove it"
}

# crates.io rejects an upload whose manifest carries no description, so every publishable crate has one.
# cargo only warns locally, which means a missing description fails at upload time -- partway through a
# workspace release, leaving some crates published at the new version and the rest behind.
deny contains msg if {
	some file in input
	is_member(file)
	file.contents.package.publish == true
	not is_string(file.contents.package.description)
	msg := sprintf("%s: crate %q is published, so it must set a description", [file.path, file.contents.package.name])
}

# An empty `features = []` on a dependency is pointless noise -- drop it.
deny contains msg if {
	some [path, name, spec] in dep
	is_object(spec)
	spec.features == []
	msg := $"{path}: dependency \"{name}\" has an empty features = []; remove it"
}

# A feature must not share its name with a dependency.
# Such a name shadows the implicit feature an optional dep creates and is confusing. generated/rust-rest is exempt --
# its generator emits a `tracing` feature beside a (non-optional) `tracing` dep.
is_dep_name(file, name) if {
	some table in {"dependencies", "dev-dependencies", "build-dependencies"}
	file.contents[table][name]
}

# A member crate's feature must not share its name with one of its dependencies.
deny contains msg if {
	some file in input
	is_member(file)
	file.path != "generated/rust-rest/Cargo.toml"
	some feat, _ in file.contents.features
	is_dep_name(file, feat)
	msg := sprintf("%s: feature %q shares its name with a dependency; rename it", [file.path, feat])
}

# No manifest may depend on a `cargo_*` / `cargo-*` crate (`cargo_metadata`, `cargo_toml`, ...).
# They drive or emulate cargo itself -- most run `cargo metadata`, which takes the workspace lock -- when code here
# needs only a manifest, which the `toml` crate reads directly.
deny contains msg if {
	some [path, name, _] in dep
	regex.match(`^cargo[-_]`, name)
	msg := $"{path}: dependency \"{name}\" is a cargo-driving crate; read the manifest with `toml` instead"
}

# Crates log through `tracing`, so none may depend on the `log` facade directly.
# A `log` record reaches the OTLP pipeline only through the binaries' bridge, flattened to a string with no fields
# or span. The one exemption is a member whose derive macro expands to `log::` paths it cannot avoid: the storage
# service's `actix-web-thiserror` `ResponseError` derive emits `log::error!`. The root entry stays for it to inherit.
log_exempt := {"services/storage/Cargo.toml"}

deny contains msg if {
	some [path, name, _] in dep
	name == "log"
	path != "Cargo.toml"
	not log_exempt[path]
	msg := $"{path}: dependency \"log\" is banned; log through `tracing` instead"
}

# tokio's `full` feature is banned on every dependency spec, the workspace table included.
# It switches on every tokio module -- process, fs, io-std, the multi-thread scheduler -- whether the crate uses
# them or not, and through feature unification it forces all of that onto every downstream user as well. Name the
# features the crate actually calls instead (`rt`, `macros`, `net`, `time`, `sync`, `signal`, ...).
deny contains msg if {
	some [path, name, spec] in dep
	name == "tokio"
	is_object(spec)
	"full" in spec.features
	msg := $"{path}: tokio's \"full\" feature is banned; list the tokio features this crate uses"
}

# The root `tracing` dependency must enable `log`.
# That way a package built alone expands the tracing macros as the workspace does. actix-codec turns `log` on in
# every workspace build; without the root stating it, a crate outside the actix tree built by itself (cargo-hack's
# per-package clippy, wasm-pack) expands them smaller, and a `#[expect(clippy::cognitive_complexity)]` the workspace
# build requires goes unfulfilled there. Members inherit the root entry, and inheriting can add features but never
# drop them, so the root is the one place to hold it.
tracing_has_log(spec) if "log" in spec.features

deny contains msg if {
	spec := root_dep.tracing
	not tracing_has_log(spec)
	msg := "Cargo.toml: workspace dependency \"tracing\" must enable its \"log\" feature, as every workspace build does"
}

# Whether a manifest declares a feature anyone picks; `default` and the `docs` switch below are not ones.
declares_feature(file) if {
	some feat, _ in file.contents.features
	not feat in {"default", "docs"}
}

# The `document-features` dependency spec, outside dev-dependencies.
# A target-scoped table counts, for a crate whose whole body is gated to one target.
feature_docs_dep(file) := file.contents.dependencies["document-features"]

feature_docs_dep(file) := spec if {
	some tgt in file.contents.target
	spec := tgt.dependencies["document-features"]
}

# Whether docs.rs builds the crate with its `docs` feature on.
# The unquoted `[package.metadata.docs.rs]` header parses as nested `docs` then `rs` tables.
docs_rs_renders(file) if file.contents.package.metadata.docs.rs["all-features"] == true

docs_rs_renders(file) if "docs" in file.contents.package.metadata.docs.rs.features

# The member crates the feature-docs contract applies to.
# generated/rust-rest is exempt: et-int-gen regenerates its src/lib.rs, which carries no `document_features!` call.
feature_docs_subject contains file if {
	some file in input
	is_member(file)
	file.path != "generated/rust-rest/Cargo.toml"
	declares_feature(file)
}

# Each way a subject crate falls short of the contract, as [path, problem] pairs.
feature_docs_problem contains [file.path, "it must depend on document-features"] if {
	some file in feature_docs_subject
	not feature_docs_dep(file)
}

feature_docs_problem contains [file.path, "its document-features dependency must be optional"] if {
	some file in feature_docs_subject
	spec := feature_docs_dep(file)
	not spec.optional == true
}

feature_docs_problem contains [file.path, "it must declare `docs = [\"dep:document-features\"]`"] if {
	some file in feature_docs_subject
	not file.contents.features.docs == ["dep:document-features"]
}

feature_docs_problem contains [file.path, "[package.metadata.docs.rs] must enable its `docs` feature"] if {
	some file in feature_docs_subject
	not docs_rs_renders(file)
}

# A crate that declares a feature must publish the feature list on docs.rs through `document-features`.
# The crate turns the `##` comment above each feature into that list; without it those comments reach no user, and
# the only way to learn what a feature does is to read the manifest. The dependency is optional behind a `docs`
# feature that docs.rs enables, so a downstream build never compiles a proc-macro that only renders documentation.
deny contains msg if {
	some [path, problem] in feature_docs_problem
	msg := $"{path}: declares features, so {problem} to publish their docs"
}

# Dependency overrides ([patch]/[replace]) belong in the root manifest, not a member crate.
# There they apply workspace-wide and stay in one place; a member can't override deps.
deny contains msg if {
	some file in input
	is_member(file)
	some table in {"patch", "replace"}
	file.contents[table]
	msg := sprintf("%s: [%s] belongs in the root manifest, not a member crate", [file.path, table])
}
