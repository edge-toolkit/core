# Each machine's benchmark results, under each OS it ran, sit in a file named for that machine and OS.
# Run with `--combine --namespace bench_results` over `libs/*/benches/results/*.yaml`, so `input` is an array of
# {path, contents}, one per results file.
#
# A results file is what `mise run bench <dir> [machine]` writes: the machine it was measured on under `machine`,
# with its `id` and the `os` it ran, and every benchmark's estimates under `benchmarks`. The same machine gives other
# results under another OS, through its other drivers, so a file is found by both, and its name must agree with what
# it records: a file renamed by hand, or one recorded under a mistyped id, would sit beside the real one as a second
# machine nobody has. The id is lowercase words joined by `-`, so it is the same file name on every OS. A file of one
# GPU's benchmarks also records that GPU under `gpu`, with an `id` of the same form, and is named for it between the
# machine and the OS, so one machine's GPUs each have a file of their own.
package bench_results

import data.paths

# A machine id: lowercase letters and digits, in words joined by single hyphens.
id_pattern := `^[a-z0-9]+(-[a-z0-9]+)*$`

# The directory a results file sits in, and its suffix; what is between them is the machine id and the OS.
results_path := `^(?:libs|services)/[^/]+/benches/results/([^/]+)\.yaml$`

# Where a results file belongs, as the denial for one elsewhere names it.
results_home := "<libs|services>/<crate>/benches/results/<machine>-<os>.yaml"

# The name a file recording machine `id` under `os` takes, and the GPU `gpu` where it records one.
expected_stem(id, "", os) := $"{id}-{os}"

expected_stem(id, gpu, os) := $"{id}-{gpu}-{os}" if gpu != ""

# Every OS a host build runs the benchmarks on, as the registry's rows name it.
oses := {"linux", "macos", "windows"}

# A results file sits in a crate's benches/results directory.
deny contains msg if {
	some file in paths.files
	not regex.match(results_path, file.path)
	msg := sprintf("%s: a benchmark results file belongs in %s", [file.path, results_home])
}

# A results file is named for the machine and OS it records, and the GPU where it records one.
deny contains msg if {
	some file in paths.files
	stem := regex.find_all_string_submatch_n(results_path, file.path, 1)[0][1]
	id := object.get(file.contents, ["machine", "id"], "<none>")
	os := object.get(file.contents, ["machine", "os"], "<none>")
	gpu := object.get(file.contents, ["gpu", "id"], "")
	expected := expected_stem(id, gpu, os)
	stem != expected
	msg := sprintf("%s: records machine %v under %v, so it must be named %v.yaml", [file.path, id, os, expected])
}

# A machine id is lowercase words joined by hyphens.
deny contains msg if {
	some file in paths.files
	id := file.contents.machine.id
	not regex.match(id_pattern, id)
	msg := sprintf("%s: machine id %q must be lowercase words joined by -", [file.path, id])
}

# A GPU id is lowercase words joined by hyphens, as a machine id is.
deny contains msg if {
	some file in paths.files
	id := file.contents.gpu.id
	not regex.match(id_pattern, id)
	msg := sprintf("%s: gpu id %q must be lowercase words joined by -", [file.path, id])
}

# A results file records the OS it ran, as one a host build runs on.
deny contains msg if {
	some file in paths.files
	os := object.get(file.contents, ["machine", "os"], "<none>")
	not os in oses
	msg := sprintf("%s: machine os %q must be one of %v", [file.path, os, sort(oses)])
}

# A results file's benchmarks are a mapping, each benchmark's name to its measures.
deny contains msg if {
	some file in paths.files
	benchmarks := object.get(file.contents, "benchmarks", {})
	not is_object(benchmarks)
	msg := sprintf("%s: benchmarks must map each benchmark's name to its measures, not be %v", [file.path, benchmarks])
}

# A results file records at least one benchmark.
deny contains msg if {
	some file in paths.files
	benchmarks := object.get(file.contents, "benchmarks", {})
	is_object(benchmarks)
	count(benchmarks) == 0
	msg := sprintf("%s: records no benchmarks", [file.path])
}
