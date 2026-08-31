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
banned := {
	"anyhow": "define a thiserror enum instead",
	"openssl": "use rustls + aws-lc-rs -- one TLS/crypto stack only",
	"openssl-sys": "use rustls + aws-lc-rs -- one TLS/crypto stack only",
	"ring": "use aws-lc-rs (transitive via rcgen only; gated in config/deny.toml)",
	"ureq": "use reqwest::blocking or reqwest -- one HTTPS stack only",
}

deny contains msg if {
	some [path, name, _] in dep
	path == "Cargo.toml"
	reason := banned[name]
	msg := sprintf("%s: banned dependency %q -- %s", [path, name, reason])
}

# Member crates: no path deps, no wildcard versions, no inline git deps.
# Pins live in the root [workspace.dependencies]; members reference them via workspace = true.
deny contains msg if {
	some [path, name, spec] in dep
	path != "Cargo.toml"
	is_object(spec)
	spec.path
	msg := sprintf("%s: dependency %q uses a path dep; use workspace = true instead", [path, name])
}

deny contains msg if {
	some [path, name, spec] in dep
	path != "Cargo.toml"
	wildcard(spec)
	msg := sprintf("%s: dependency %q uses a wildcard version; pin via [workspace.dependencies]", [path, name])
}

deny contains msg if {
	some [path, name, spec] in dep
	path != "Cargo.toml"
	is_object(spec)
	spec.git
	msg := sprintf("%s: dependency %q is an inline git dep; pin via [workspace.dependencies]", [path, name])
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

deny contains msg if {
	some file in input
	is_member(file)
	name := file.contents.package.name
	not contains(name, "int-")
	not file.contents.package.publish == true
	msg := sprintf("%s: crate %q must set publish = true; add an int- marker to keep it internal", [file.path, name])
}

# An empty `features = []` on a dependency is pointless noise -- drop it.
deny contains msg if {
	some [path, name, spec] in dep
	is_object(spec)
	spec.features == []
	msg := sprintf("%s: dependency %q has an empty features = []; remove it", [path, name])
}

# A feature must not share its name with a dependency.
# Such a name shadows the implicit feature an optional dep creates and is confusing. generated/rust-rest is exempt --
# its generator emits a `tracing` feature beside a (non-optional) `tracing` dep.
is_dep_name(file, name) if {
	some table in {"dependencies", "dev-dependencies", "build-dependencies"}
	file.contents[table][name]
}

deny contains msg if {
	some file in input
	is_member(file)
	file.path != "generated/rust-rest/Cargo.toml"
	some feat, _ in file.contents.features
	is_dep_name(file, feat)
	msg := sprintf("%s: feature %q shares its name with a dependency; rename it", [file.path, feat])
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
