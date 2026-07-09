# GitHub Actions composite-action policy, evaluated per file over each `.github/actions/*/action.yaml`.
# Sister to `gha`/`gha_combined` (which cover workflow YAML); split into its own package because composite-action
# schemas differ from workflows (no `jobs.*`, `runs.steps[]` instead of `jobs.<id>.steps[]`, no `defaults.run.shell`
# support). Selected with `--namespace gha_action`.
package gha_action

# Composite-action `run:` steps require an explicit `shell:`.
# GHA's `defaults.run.shell` isn't honored inside composite actions, so every step has to declare its own shell.
# Enforce the same explicit form the `gha` rule requires for workflow `defaults.run.shell`:
# `bash --noprofile --norc -euo pipefail {0}`. The GHA shorthand `shell: bash` is rejected because it expands to
# `bash --noprofile --norc -e -o pipefail {0}` (missing `-u`); the full form catches misspelled-variable bugs in every
# step uniformly. PowerShell-specific work that can't be expressed in Git Bash on Windows must live in a workflow step
# (with the workflow-default shell policy) instead.
required_shell := "bash --noprofile --norc -euo pipefail {0}"

deny contains msg if {
	is_composite_action
	some step in input.runs.steps
	step.run
	step.shell != required_shell
	msg := sprintf(
		"step %q uses shell %q; composite-action steps must use shell %q",
		[step.name, step.shell, required_shell],
	)
}

is_composite_action if {
	input.runs.using == "composite"
}

# Every actions/upload-artifact step must set with.if-no-files-found: error.
# Composite-action steps get their own copy of this guard because they live in runs.steps[], which the workflow
# policy's jobs.*.steps[] iteration never reaches; the action's "warn" default silently uploads nothing when a path
# matches no files, so a missing artifact reads as success. "error" fails the run at upload time instead.
deny contains msg if {
	is_composite_action
	some step in input.runs.steps
	startswith(step.uses, "actions/upload-artifact")
	object.get(step, ["with", "if-no-files-found"], "warn") != "error"
	msg := sprintf("step %q: actions/upload-artifact must set with.if-no-files-found: error", [step.name])
}
