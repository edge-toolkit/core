# Cross-file invariants, run with `--namespace cross`.
# Evaluated over conftest's `--combine` input (an array of {path, contents}).
package cross

# The mise `github:wasm-bindgen` pin must equal the wasm-bindgen package version in Cargo.lock.
# wasm-pack requires the wasm-bindgen CLI to match the crate version exactly; when they match it uses the on-PATH
# (mise) binary, otherwise it downloads its own. Keeping them equal avoids that download.
mise_pin := pin if {
	some file in input
	endswith(file.path, ".mise/config.toml")
	pin := file.contents.tools["github:wasm-bindgen/wasm-bindgen"]
}

lock_version := ver if {
	some file in input
	endswith(file.path, "Cargo.lock")
	some pkg in file.contents.package
	pkg.name == "wasm-bindgen"
	ver := pkg.version
}

# The mise wasm-bindgen pin must match the wasm-bindgen version Cargo.lock resolves.
deny contains msg if {
	mise_pin != lock_version
	msg := $"wasm-bindgen: mise pin \"{mise_pin}\" != Cargo.lock \"{lock_version}\"; bump the pin in .mise/config.toml"
}
