# .mise/config*.toml policy, evaluated over conftest's `--combine` input (an array
# of {path, contents}). Run with `--namespace mise` (or `--all-namespaces`).
package mise

is_mise(file) if startswith(file.path, ".mise/config")

# A task `run` must be a string, not an array: taplo's reorder_arrays would
# re-sort the commands of an array form and scramble the sequence.
deny contains msg if {
	some file in input
	is_mise(file)
	some name, task in file.contents.tasks
	is_array(task.run)
	msg := sprintf("%s: task %q run must be a string, not an array", [file.path, name])
}

# A multiline `run` must use `shell = "bash -euo pipefail -c"` so a failing
# command fails the task instead of being masked.
deny contains msg if {
	some file in input
	is_mise(file)
	some name, task in file.contents.tasks
	is_string(task.run)
	contains(task.run, "\n")
	not task.shell == "bash -euo pipefail -c"
	msg := sprintf("%s: task %q has a multiline run; set shell = \"bash -euo pipefail -c\"", [file.path, name])
}

# Task descriptions must be single-line (keep them under the 120-char limit).
deny contains msg if {
	some file in input
	is_mise(file)
	some name, task in file.contents.tasks
	is_string(task.description)
	contains(task.description, "\n")
	msg := sprintf("%s: task %q description must be a single line", [file.path, name])
}

# `cargo:` tools build from source; prefer a prebuilt backend. Allowlist the two
# that have no prebuilt binary.
allowed_cargo_tool := {"cargo:cargo-expand", "cargo:dart-typegen"}

deny contains msg if {
	some file in input
	is_mise(file)
	some name, _ in file.contents.tools
	startswith(name, "cargo:")
	not allowed_cargo_tool[name]
	msg := sprintf("%s: tool %q builds from source; use a prebuilt backend", [file.path, name])
}

# `ubi:` is deprecated; the `http:` backend replaces it.
deny contains msg if {
	some file in input
	is_mise(file)
	some name, _ in file.contents.tools
	startswith(name, "ubi:")
	msg := sprintf("%s: tool %q uses the deprecated ubi backend; use http: instead", [file.path, name])
}
