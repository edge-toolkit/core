# The mise `cuda_crates` var must name exactly the workspace crates whose `Cargo.toml` declares a `cuda` feature.
# The clippy tasks turn on `<crate>/cuda` for each crate it names, so one it misses has its `cuda` code linted nowhere,
# and one it names without the feature fails every clippy run on a host with an NVIDIA GPU. Run with
# `--namespace cross` over conftest's `--combine` input.
package cross

import data.paths

# Each crate the mise `cuda_crates` var names.
listed_cuda_crates contains name if {
	some file in paths.files
	file.path == ".mise/config.toml"
	some name in split(file.contents.vars.cuda_crates, " ")
	name != ""
}

# Each workspace crate whose manifest declares a `cuda` feature, beside that manifest's path.
featured_cuda_crates[name] := file.path if {
	some file in paths.files
	endswith(file.path, "Cargo.toml")
	file.contents.features.cuda
	name := file.contents.package.name
}

# A crate with a `cuda` feature must be named by `cuda_crates`, or no clippy task ever turns the feature on.
deny contains msg if {
	some name, path in featured_cuda_crates
	not name in listed_cuda_crates
	msg := $"{path}: {name} has a `cuda` feature, so .mise/config.toml's `cuda_crates` var must name it"
}

# A crate `cuda_crates` names must have a `cuda` feature, or cargo refuses the `--features` the clippy tasks pass.
deny contains msg if {
	some name in listed_cuda_crates
	not featured_cuda_crates[name]
	msg := $".mise/config.toml: `cuda_crates` names {name}, which has no crate with a `cuda` feature"
}
