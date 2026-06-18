# GitHub Actions workflow policy, evaluated per file: conftest reads each .yaml
# independently (no --combine, since there are no cross-file YAML rules).
# Replicates the gha-* ast-grep rules; running both is fine. Selected with
# `--namespace gha`, so it only runs against workflow YAML, never the TOML inputs.
package gha

# Every workflow must set the default run shell so steps run in bash on every
# runner (Windows included) without a per-step `shell:`. GHA's bare `shell: bash`
# already implies `bash --noprofile --norc -e -o pipefail {0}`; we expand it to
# the explicit `-euo pipefail` form so every step also gets `-u` (any
# reference to an unset variable errors out -- catches misspelled vars).
required_shell := "bash --noprofile --norc -euo pipefail {0}"

deny contains msg if {
	not input.defaults.run.shell == required_shell
	msg := sprintf("workflow must set defaults.run.shell: %q (gets -euo pipefail on every step)", [required_shell])
}

# Steps must not override the shell -- rely on the workflow default (write the
# step in bash rather than switching to PowerShell on Windows runners).
deny contains msg if {
	some name, job in input.jobs
	some step in job.steps
	step.shell
	msg := sprintf("job %q sets shell: on a step; use the workflow default", [name])
}

# Every workflow must declare MISE_ENV at the workflow level so the set of
# loaded language envs is visible at a glance (no per-job `mise run
# print-all-langs` runtime resolution). The matching `Show MISE_ENV` step
# in each job echoes the value into the CI log.
deny contains msg if {
	not input.env.MISE_ENV
	msg := "workflow must set top-level env.MISE_ENV (the comma-separated language list)"
}

# The MISE_ENV VALUE must be the full guest-language set so every CI run
# exercises the same toolchain footprint as a local `mise install`. The
# docker-windows workflow's Nano lane drops `python` via a matrix-specific
# build-arg override; the workflow-level value still matches the standard.
expected_mise_env := "dart,dotnet,java,python,rust,zig"

deny contains msg if {
	input.env.MISE_ENV != expected_mise_env
	msg := sprintf(
		"workflow %q must set env.MISE_ENV to %q (the full guest-language set)",
		[input.name, expected_mise_env],
	)
}

# `DOCKER_BUILDKIT=1` must not be set in the docker-windows workflow: GHA
# Windows runners ship without the `buildx` CLI, so the very first
# `docker build` aborts with the literal error
#
#   ERROR: BuildKit is enabled but the buildx component is missing or broken.
#          Install the buildx component to build images with BuildKit:
#          https://docs.docker.com/go/buildx/
#
# (Captured 2026-06-18 on the docker-windows lane.) BuildKit cache mounts
# (`RUN --mount=type=cache,...`) require DOCKER_BUILDKIT=1; until buildx is
# installable on Windows runners, enabling it just trades a working build
# for a guaranteed fail-fast. Anchored to docker-windows by `input.name` so
# other workflows are unaffected. Checks all three scopes the env can be
# set from: workflow-level, job-level, step-level.
buildkit_error_hint := "GHA Windows runners ship without buildx; build aborts"

deny contains msg if {
	input.name == "docker-windows"
	input.env.DOCKER_BUILDKIT
	msg := sprintf(
		"docker-windows must not set DOCKER_BUILDKIT at workflow scope (%s)",
		[buildkit_error_hint],
	)
}

deny contains msg if {
	input.name == "docker-windows"
	some name, job in input.jobs
	job.env.DOCKER_BUILDKIT
	msg := sprintf(
		"docker-windows job %q must not set DOCKER_BUILDKIT (%s)",
		[name, buildkit_error_hint],
	)
}

deny contains msg if {
	input.name == "docker-windows"
	some name, job in input.jobs
	some step in job.steps
	step.env.DOCKER_BUILDKIT
	msg := sprintf(
		"docker-windows job %q must not set DOCKER_BUILDKIT on a step (%s)",
		[name, buildkit_error_hint],
	)
}

# When a workflow uses a local composite action (./.github/actions/<name>) and
# also filters its triggers with a paths: list, that list must include the
# composite action's location. Otherwise edits to the composite action won't
# re-trigger this workflow on a PR that changes it. Workflows without a paths
# filter already trigger on every PR/push and are skipped by this rule.
local_actions_used contains action_dir if {
	some job in input.jobs
	some step in job.steps
	startswith(step.uses, "./.github/actions/")

	# Strip the leading "./" to get the repo-relative dir (matches gitignore-style
	# path filters in `on.<event>.paths`).
	action_dir := substring(step.uses, 2, -1)
}

# Paths array configured for a trigger event, if any.
trigger_paths(event) := paths if {
	paths := input.on[event].paths
	is_array(paths)
}

# True if `paths` contains an entry that covers `action_dir` (either the
# directory glob `<dir>/**` or any narrower entry under the dir, e.g.
# `<dir>/action.yml`).
paths_cover_dir(paths, action_dir) if {
	some p in paths
	startswith(p, action_dir)
}

deny contains msg if {
	some action_dir in local_actions_used
	some event in ["pull_request", "pull_request_target", "push"]
	paths := trigger_paths(event)
	not paths_cover_dir(paths, action_dir)
	msg := sprintf(
		"workflow uses ./%s but on.%s.paths doesn't include it (e.g. %q) -- edits won't re-trigger this workflow",
		[action_dir, event, sprintf("%s/**", [action_dir])],
	)
}
