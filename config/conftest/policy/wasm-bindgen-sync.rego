# Cross-file invariants, evaluated over conftest's `--combine` input (an array of
# {path, contents}). Run with `--namespace cross`.
package cross

# The mise `github:wasm-bindgen` pin must equal the wasm-bindgen package version
# in Cargo.lock. wasm-pack requires the wasm-bindgen CLI to match the crate
# version exactly; when they match it uses the on-PATH (mise) binary, otherwise it
# downloads its own. Keeping them equal avoids that download.
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

deny contains msg if {
	mise_pin != lock_version
	msg := sprintf(
		"wasm-bindgen: mise pin %q != Cargo.lock %q; bump the pin in .mise/config.toml",
		[mise_pin, lock_version],
	)
}
