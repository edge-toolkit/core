# Unit tests for the local-action input and install-root cross-checks, run by `conftest verify`.
#
# Every real caller already passes declared inputs, so the real task cannot tell a clean repo from a rule that
# stopped resolving actions -- which is exactly what a path-separator change once did on Windows.
package gha_uses_test

import data.gha_uses

action := {
	"path": ".github/actions/install-mise/action.yaml",
	"contents": {
		"inputs": {"install-action-tools": {}},
		"runs": {"steps": [{"run": `echo 'MISE_INSTALLS_DIR=C:\i' >> "$GITHUB_ENV"`}]},
	},
}

calling(inputs) := [action, {
	"path": ".github/workflows/probe.yaml",
	"contents": {"jobs": {"build": {"steps": [{"uses": "$/.github/actions/install-mise", "with": inputs}]}}},
}]

undeclared(msgs) if {
	some msg in msgs
	contains(msg, "declares no such input")
}

misrooted(msgs) if {
	some msg in msgs
	contains(msg, "must sit under the MISE_INSTALLS_DIR")
}

test_a_declared_input_is_accepted if {
	not undeclared(gha_uses.deny) with input as calling({"install-action-tools": "cargo-llvm-cov"})
}

test_an_undeclared_input_is_denied if {
	undeclared(gha_uses.deny) with input as calling({"install-action": "cargo-llvm-cov"})
}

test_windows_separators_still_resolve_the_action if {
	windows := object.union(action, {"path": `.github\actions\install-mise\action.yaml`})
	files := array.concat([windows], array.slice(calling({"install-action": "x"}), 1, 2))
	undeclared(gha_uses.deny) with input as files
}

test_a_composite_action_calling_another_is_checked if {
	caller := {
		"path": ".github/actions/install-mise-tools/action.yaml",
		"contents": {"runs": {"steps": [{"uses": "$/.github/actions/install-mise", "with": {"typo": "x"}}]}},
	}
	undeclared(gha_uses.deny) with input as [action, caller]
}

shell_workflow(shell) := [action, {
	"path": ".github/workflows/probe.yaml",
	"contents": {"jobs": {"build": {"defaults": {"run": {"shell": shell}}, "steps": []}}},
}]

test_a_busybox_path_under_the_exported_root_is_accepted if {
	shell := `${{ runner.os == 'Windows' && 'C:\i\http-busybox\1.37.0\busybox.exe sh -e {0}' || 'bash' }}`
	not misrooted(gha_uses.deny) with input as shell_workflow(shell)
}

test_a_busybox_path_under_another_root_is_denied if {
	shell := `C:\mise\installs\http-busybox\1.37.0\busybox.exe sh -e {0}`
	misrooted(gha_uses.deny) with input as shell_workflow(shell)
}
