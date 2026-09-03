# Unit tests for the generated_trees policy, run by `conftest verify`.
#
# These exist for one reason the other policies do not need: this policy compares conftest's reported file paths
# against paths written in a config file, and conftest reports them the way the host OS hands them over. A
# directory walked on Windows yields `config\semgrep\x.yaml` where macOS and Linux yield `config/semgrep/x.yaml`,
# so the Windows behaviour cannot be exercised by running the real task on a developer machine or on the Linux
# lane. Feeding synthetic input here covers that lane's shape from any OS.
package generated_trees_test

import data.generated_trees

# A declaration fixture covering both anchor styles the real file uses.
declaration := {
	"path": "config/generated-trees.toml",
	"contents": {
		"anchor": {
			"config/ast-grep/": "{tree}/**",
			"config/semgrep/": "/{tree}/**",
		},
		"tree": {"generated": {"required_in": [
			"config/ast-grep/rules/no-map-err.yaml",
			"config/semgrep/no-todo.yaml",
		]}},
	},
}

# A governed config carrying the exclusion its anchor calls for, addressed with `sep` as the path separator.
governed(sep, path, exclusion) := {
	"path": replace(path, "/", sep),
	"contents": {"paths": {"exclude": [exclusion]}},
}

posix_input := [
	declaration,
	governed("/", "config/ast-grep/rules/no-map-err.yaml", "generated/**"),
	governed("/", "config/semgrep/no-todo.yaml", "/generated/**"),
]

# The same tree as a Windows lane reports it.
# The declaration keeps the slashes it was typed with on the command line, while the walked configs arrive with
# backslashes -- the mismatch that turned every forward check red on that lane.
windows_input := [
	declaration,
	governed("\\", "config/ast-grep/rules/no-map-err.yaml", "generated/**"),
	governed("\\", "config/semgrep/no-todo.yaml", "/generated/**"),
]

test_posix_paths_satisfy_the_declaration if {
	count(generated_trees.deny) == 0 with input as posix_input
}

test_windows_paths_satisfy_the_declaration if {
	count(generated_trees.deny) == 0 with input as windows_input
}

# The Windows shape must still be able to fail, or the normalisation would just be silencing the whole policy.
test_windows_paths_still_catch_a_missing_exclusion if {
	broken := [
		declaration,
		governed("\\", "config/ast-grep/rules/no-map-err.yaml", "generated/**"),
		governed("\\", "config/semgrep/no-todo.yaml", "/somewhere-else/**"),
	]

	messages := generated_trees.deny with input as broken
	count(messages) == 1
	contains(messages[_], "config/semgrep/no-todo.yaml")
}

# An undeclared exclusion must still be caught through a backslash path.
test_windows_paths_still_catch_an_undeclared_exclusion if {
	extra := [
		declaration,
		governed("\\", "config/ast-grep/rules/no-map-err.yaml", "generated/**"),
		governed("\\", "config/semgrep/no-todo.yaml", "/generated/**"),
		governed("\\", "config/semgrep/no-non-ascii.yaml", "/generated/**"),
	]

	messages := generated_trees.deny with input as extra
	count(messages) == 1
	contains(messages[_], "not in its `required_in` list")
}
