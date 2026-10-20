# Which crates may depend on which, over the same `--combine` Cargo.toml input as the rest of `package cargo`.
# It shares that package, and so the `dep` triples and `is_member` from the policy beside it, and runs in the same
# `--namespace cargo` evaluation.
package cargo

# An internal crate is one whose name carries the int- marker, as a prefix or after this repo's own `et-`.
# A plain substring test would also catch third-party names such as `color-print-proc-macro`.
internal_crate_name(name) if startswith(name, "int-")

internal_crate_name(name) if startswith(name, "et-int-")

# The package a dependency entry names: its own `package` rename, else the one on the workspace entry it inherits.
# A key can name a different package -- `other = { package = "et-org" }` -- so the key alone would let a rule be
# evaded by renaming. Members must inherit through `workspace = true`, so the rename usually sits on the root entry.
dep_package(_, spec) := spec.package if dep_package_renamed(spec)

dep_package(name, spec) := workspace_package_rename[name] if {
	not dep_package_renamed(spec)
	is_object(spec)
	spec.workspace == true
}

dep_package(name, spec) := name if {
	not dep_package_renamed(spec)
	not inherits_rename(name, spec)
}

dep_package_renamed(spec) if {
	is_object(spec)
	is_string(spec.package)
}

inherits_rename(name, spec) if {
	is_object(spec)
	spec.workspace == true
	workspace_package_rename[name]
}

# Each root [workspace.dependencies] key that renames its package, mapped to the package it names.
workspace_package_rename[name] := spec.package if {
	some file in input
	file.path == "Cargo.toml"
	some name, spec in file.contents.workspace.dependencies
	is_object(spec)
	is_string(spec.package)
}

# No crate may depend on an internal one.
# An internal crate is unpublished by construction, so a dependency on it either breaks the dependent's publish or
# ties an unpublished build tool into a crate's graph.
deny contains msg if {
	some [path, name, spec] in dep
	path != "Cargo.toml"
	pkg := dep_package(name, spec)
	internal_crate_name(pkg)
	msg := $"{path}: depends on internal crate \"{pkg}\"; no crate may depend on an int- crate"
}

# Crates written to be used outside this repository, which therefore take no dependency on this repo's own `et-` ones.
# Pulling one in would drag this project's internals into every consumer's build.
standalone_crates := {"et-actix-web-config"}

# A standalone crate must not depend on any of this repo's `et-` crates.
deny contains msg if {
	some file in input
	is_member(file)
	crate := file.contents.package.name
	crate in standalone_crates
	some [path, name, spec] in dep
	path == file.path
	pkg := dep_package(name, spec)
	startswith(pkg, "et-")
	msg := $"{path}: standalone crate \"{crate}\" must not depend on this repo's \"{pkg}\""
}

# No crate under services/ may depend on clap.
# A service reads all of its configuration from the environment through serde-env: that is the one interface every
# way it is deployed here -- compose, k3s, process-compose, mise -- sets the same way, where a flag has to be threaded
# through each one's command line separately, and it keeps the whole surface in one generated list in `HELP.md`.
deny contains msg if {
	some [path, name, spec] in dep
	startswith(path, "services/")
	dep_package(name, spec) == "clap"
	msg := $"{path}: services take no command-line arguments; read the setting from the environment through serde-env"
}
