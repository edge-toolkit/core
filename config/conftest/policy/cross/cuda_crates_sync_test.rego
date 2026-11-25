# Unit tests for the `cuda_crates` var against the crates with a `cuda` feature, run by `conftest verify`.
#
# The real task only ever sees a list that already agrees, which looks the same as a rule that never matches.
package cross_test

import data.cross

cuda_input(listed, manifests) := array.concat(
	[{"path": ".mise/config.toml", "contents": {"vars": {"cuda_crates": listed}}}],
	manifests,
)

manifest(path, name, features) := {"path": path, "contents": {"package": {"name": name}, "features": features}}

probe := manifest("utilities/gpu-probe/Cargo.toml", "et-gpu-probe-server", {"cuda": ["dep:cuda-core"]})

runner := manifest("services/ws-wasi-runner/Cargo.toml", "et-ws-wasi-runner", {"cuda": ["nn"], "nn": []})

plain := manifest("libs/gpu-probe/Cargo.toml", "et-gpu-probe", {"native": []})

cuda_reports(msgs, fragment) if {
	some msg in msgs
	contains(msg, fragment)
}

test_a_list_naming_every_cuda_crate_is_accepted if {
	msgs := cross.deny with input as cuda_input("et-gpu-probe-server et-ws-wasi-runner", [probe, runner, plain])
	not cuda_reports(msgs, "cuda")
}

test_a_cuda_crate_left_out_is_flagged if {
	msgs := cross.deny with input as cuda_input("et-gpu-probe-server", [probe, runner, plain])
	cuda_reports(msgs, "services/ws-wasi-runner/Cargo.toml: et-ws-wasi-runner has a `cuda` feature")
}

test_a_crate_without_the_feature_is_flagged if {
	listed := "et-gpu-probe-server et-ws-wasi-runner et-gpu-probe"
	msgs := cross.deny with input as cuda_input(listed, [probe, runner, plain])
	cuda_reports(msgs, "`cuda_crates` names et-gpu-probe, which has no crate with a `cuda` feature")
}
