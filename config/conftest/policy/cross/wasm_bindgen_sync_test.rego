# Unit tests for the wasm-bindgen pin cross-check, run by `conftest verify`.
#
# The real task only ever sees a pin that already agrees with Cargo.lock, which looks the same as a rule that never
# matches.
package cross_test

import data.cross

bindgen_input(pin, lock_version) := [
	{"path": ".mise/config.toml", "contents": {"tools": {"github:wasm-bindgen/wasm-bindgen": pin}}},
	{"path": "Cargo.lock", "contents": {"package": [
		{"name": "wasm-bindgen-shared", "version": "0.0.1"},
		{"name": "wasm-bindgen", "version": lock_version},
	]}},
]

bindgen_drift(msgs) if {
	some msg in msgs
	startswith(msg, "wasm-bindgen: mise pin")
}

test_a_pin_matching_the_lock_is_accepted if {
	not bindgen_drift(cross.deny) with input as bindgen_input("0.2.130", "0.2.130")
}

test_a_pin_behind_the_lock_is_flagged if {
	msgs := cross.deny with input as bindgen_input("0.2.129", "0.2.130")
	some msg in msgs
	contains(msg, "mise pin \"0.2.129\" != Cargo.lock \"0.2.130\"")
}

test_a_windows_config_path_is_still_read if {
	files := bindgen_input("0.2.129", "0.2.130")
	windows := [object.union(files[0], {"path": `.mise\config.toml`}), files[1]]
	bindgen_drift(cross.deny) with input as windows
}

test_a_pin_matching_only_a_sibling_crate_is_flagged if {
	bindgen_drift(cross.deny) with input as bindgen_input("0.0.1", "0.2.130")
}
