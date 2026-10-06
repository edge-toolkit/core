# Cross-file invariant, run with `--namespace cross` over conftest's `--combine` input.
package cross

# The mise `python` pin, in either the string or the `{ version = ... }` form.
mise_python := version if {
	some file in input
	endswith(file.path, ".mise/config.toml")
	version := file.contents.tools.python
	is_string(version)
}

mise_python := version if {
	some file in input
	endswith(file.path, ".mise/config.toml")
	version := file.contents.tools.python.version
}

pyrefly_files contains file if {
	some file in input
	endswith(file.path, "config/pyrefly.toml")
}

# pyrefly must pin the Python version it type-checks against.
# Left unset, it checks against whichever interpreter the host's PATH yields first, so the same code passes on one
# OS and fails on another.
deny contains msg if {
	some file in pyrefly_files
	not file.contents["python-version"]
	msg := $"{file.path}: python-version is unset; pin it to the mise python \"{mise_python}\""
}

# pyrefly's pinned Python version must equal the mise `python` pin, the version every lane installs.
deny contains msg if {
	some file in pyrefly_files
	pinned := file.contents["python-version"]
	pinned != mise_python
	msg := $"{file.path}: python-version \"{pinned}\" != mise python \"{mise_python}\"; bump it in lockstep"
}
