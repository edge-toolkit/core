# Unit tests for the benchmark results file names, run by `conftest verify`.
package bench_results_test

import data.bench_results

# A results file at `path` recording machine `id` under `os`, and one benchmark.
results(path, id, os) := {
	"path": path,
	"contents": {"machine": {"id": id, "os": os}, "benchmarks": {"probe_sets/shaders": {"latency": {"value": 1.0}}}},
}

test_a_file_named_for_its_machine_and_os_passes if {
	named := results("libs/gpu-probe/benches/results/dell-precision-5570-linux.yaml", "dell-precision-5570", "linux")
	count(bench_results.deny) == 0 with input as [named]
}

test_a_service_crates_results_file_passes if {
	named := results("services/ws-test-server/benches/results/dell-linux.yaml", "dell", "linux")
	count(bench_results.deny) == 0 with input as [named]
}

test_a_file_outside_libs_and_services_is_refused if {
	msgs := bench_results.deny with input as [results("utilities/cli/benches/results/dell-linux.yaml", "dell", "linux")]
	count(msgs) == 1
	some msg in msgs
	contains(msg, "belongs in <libs|services>/<crate>/benches/results/<machine>-<os>.yaml")
}

# A results file at `path` recording GPU `gpu` of machine `id` under `os`, and one benchmark.
gpu_results(path, id, gpu, os) := {
	"path": path,
	"contents": {
		"machine": {"id": id, "os": os},
		"gpu": {"id": gpu, "name": "Quadro M2000 (vulkan)"},
		"benchmarks": {"gpus/clear_signature": {"latency": {"value": 1.0}}},
	},
}

test_a_gpus_file_named_for_its_machine_gpu_and_os_passes if {
	path := "libs/gpu-probe/benches/results/desktop-quadro-m2000-vulkan-linux.yaml"
	named := gpu_results(path, "desktop", "quadro-m2000-vulkan", "linux")
	count(bench_results.deny) == 0 with input as [named]
}

test_a_benchmark_named_for_its_adapter_is_refused if {
	path := "libs/gpu-probe/benches/results/desktop-linux.yaml"
	benchmarks := {"gpus/open": {"latency": {"value": 1.0}}, "gpus/signature/Quadro M2000 (vulkan)": {"latency": {}}}
	unsplit := {"path": path, "contents": {"machine": {"id": "desktop", "os": "linux"}, "benchmarks": benchmarks}}
	msgs := bench_results.deny with input as [unsplit]
	count(msgs) == 1
	some msg in msgs
	contains(msg, "\"gpus/signature/Quadro M2000 (vulkan)\" names its adapter")
}

test_a_gpus_file_named_for_its_machine_alone_is_refused if {
	unnamed := gpu_results("libs/gpu-probe/benches/results/desktop-linux.yaml", "desktop", "quadro-m2000-vulkan", "linux")
	msgs := bench_results.deny with input as [unnamed]
	count(msgs) == 1
	some msg in msgs
	contains(msg, "must be named desktop-quadro-m2000-vulkan-linux.yaml")
}

test_a_gpu_id_that_is_not_lowercase_words_is_refused if {
	path := "libs/gpu-probe/benches/results/desktop-Quadro_M2000-linux.yaml"
	capitalised := gpu_results(path, "desktop", "Quadro_M2000", "linux")
	msgs := bench_results.deny with input as [capitalised]
	count(msgs) == 1
	some msg in msgs
	contains(msg, "gpu id \"Quadro_M2000\" must be lowercase words joined by -")
}

test_the_same_machine_under_two_oses_is_two_files if {
	linux := results("libs/gpu-probe/benches/results/dell-linux.yaml", "dell", "linux")
	windows := results("libs/gpu-probe/benches/results/dell-windows.yaml", "dell", "windows")
	count(bench_results.deny) == 0 with input as [linux, windows]
}

test_a_file_named_for_another_machine_is_refused if {
	renamed := results("libs/gpu-probe/benches/results/old-name-linux.yaml", "dell-precision-5570", "linux")
	msgs := bench_results.deny with input as [renamed]
	count(msgs) == 1
	some msg in msgs
	contains(msg, "must be named dell-precision-5570-linux.yaml")
}

test_a_file_named_without_its_os_is_refused if {
	unnamed := results("libs/gpu-probe/benches/results/dell.yaml", "dell", "linux")
	msgs := bench_results.deny with input as [unnamed]
	count(msgs) == 1
	some msg in msgs
	contains(msg, "must be named dell-linux.yaml")
}

test_a_file_with_no_machine_is_refused if {
	no_machine := {"path": "libs/gpu-probe/benches/results/laptop-linux.yaml", "contents": {"benchmarks": {"a": {}}}}
	msgs := bench_results.deny with input as [no_machine]
	some msg in msgs
	contains(msg, "records machine <none> under <none>")
}

test_an_id_that_is_not_lowercase_words_is_refused if {
	capitalised := results("libs/gpu-probe/benches/results/Dell_5570-linux.yaml", "Dell_5570", "linux")
	msgs := bench_results.deny with input as [capitalised]
	count(msgs) == 1
	some msg in msgs
	contains(msg, "must be lowercase words joined by -")
}

test_an_os_a_host_build_does_not_run_on_is_refused if {
	msgs := bench_results.deny with input as [results("libs/gpu-probe/benches/results/dell-beos.yaml", "dell", "beos")]
	count(msgs) == 1
	some msg in msgs
	contains(msg, `machine os "beos" must be one of`)
}

test_a_file_outside_a_results_directory_is_refused if {
	msgs := bench_results.deny with input as [results("libs/gpu-probe/benches/dell-linux.yaml", "dell", "linux")]
	count(msgs) == 1
	some msg in msgs
	contains(msg, "belongs in <libs|services>/<crate>/benches/results/<machine>-<os>.yaml")
}

# A results file named for its machine and OS, whose `benchmarks` is `value`.
benchmarks_file(value) := {
	"path": "libs/gpu-probe/benches/results/dell-linux.yaml",
	"contents": {"machine": {"id": "dell", "os": "linux"}, "benchmarks": value},
}

test_benchmarks_given_as_a_string_are_refused if {
	msgs := bench_results.deny with input as [benchmarks_file("lost")]
	count(msgs) == 1
	some msg in msgs
	contains(msg, "benchmarks must map each benchmark's name to its measures")
}

test_benchmarks_given_as_a_list_are_refused if {
	msgs := bench_results.deny with input as [benchmarks_file(["a", "b"])]
	count(msgs) == 1
	some msg in msgs
	contains(msg, "benchmarks must map each benchmark's name to its measures")
}

test_a_file_recording_no_benchmarks_is_refused if {
	contents := {"machine": {"id": "dell", "os": "linux"}, "benchmarks": {}}
	empty := {"path": "libs/gpu-probe/benches/results/dell-linux.yaml", "contents": contents}
	msgs := bench_results.deny with input as [empty]
	count(msgs) == 1
	some msg in msgs
	contains(msg, "records no benchmarks")
}
