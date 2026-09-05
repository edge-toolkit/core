# Cross-file check: every input passed to a LOCAL composite action must be one that the action declares.
# Run with `--namespace gha_uses` over the workflow YAML plus every .github/actions/*/action.yaml, combined
# (--combine) so a single evaluation resolves each `uses: ./.github/actions/<name>` against that action's `inputs:`.
# GitHub Actions only emits a *warning* -- never an error -- when a caller passes a `with:` key the target action
# does not declare, so a typo'd input name silently defaults to "" and the intended value is dropped. (Concrete
# regression: install-mise-tools passed `install-action:` to install-mise, whose input is `install-action-tools`,
# so co-installed tools like cargo-llvm-cov silently never installed and the coverage job failed with
# `no such command: llvm-cov`.) This promotes that class of typo to a hard failure.
package gha_uses

# Path separators, normalised so the Windows lane compares equal to the forward-slash paths written here.
# conftest reports each path the way the OS handed it over, so a rule splitting or ending on `/` sees
# `.github\actions\x\action.yaml` on Windows and matches nothing. That fails open: the namespace reports
# clean there while checking nothing, so a violation only ever surfaces on the Linux lanes.
normalised(path) := replace(path, "\\", "/")

# action dir name -> set of declared input keys, one entry per parsed .github/actions/<name>/action.yaml.
declared_inputs[name] := names if {
	some file in input
	parts := split(normalised(file.path), "/")
	n := count(parts)
	parts[n - 1] == "action.yaml"
	name := parts[n - 2]
	names := object.keys(object.get(file.contents, "inputs", {}))
}

# {path, step} for every step in a workflow job (jobs.<id>.steps[]) ...
caller_steps contains entry if {
	some file in input
	some job in file.contents.jobs
	some step in job.steps
	entry := {"path": file.path, "step": step}
}

# ... and every step in a composite action's runs block (runs.steps[]).
caller_steps contains entry if {
	some file in input
	some step in file.contents.runs.steps
	entry := {"path": file.path, "step": step}
}

deny contains msg if {
	some entry in caller_steps
	step := entry.step
	startswith(step.uses, "./.github/actions/")
	name := trim_prefix(step.uses, "./.github/actions/")
	declared := declared_inputs[name]
	some key, _ in step.with
	not declared[key]
	msg := sprintf(
		"%s: step passes input %q to local action %q, which declares no such input (GHA only warns -- treat as error)",
		[entry.path, key, name],
	)
}

# A workflow's hard-coded Windows tool paths must sit under the install root install-mise exports.
# A job's `defaults.run.shell` is resolved before any step runs, so it cannot read MISE_INSTALLS_DIR and has
# to spell the root out; nothing then stops the two drifting apart. When they do, the runner cannot find the
# shell and every step of the job dies at once on `Second path fragment must not be a drive or UNC name`,
# which names neither mise nor the path at fault -- so this pins the literal to the exported value.
mise_installs_dir := dir if {
	some file in input
	endswith(normalised(file.path), "install-mise/action.yaml")
	some step in file.contents.runs.steps
	some m in regex.find_all_string_submatch_n(`MISE_INSTALLS_DIR=([^'"\s]+)`, step.run, -1)
	dir := m[1]
}

# Every workflow string naming a mise-installed Windows tool, keyed on the install dir of the shell we pin.
windows_tool_path contains entry if {
	some file in input
	endswith(file.path, ".yaml")
	walk(file.contents, [_, value])
	is_string(value)
	contains(value, "http-busybox")
	entry := {"path": file.path, "value": value}
}

# Matched with `contains` rather than `startswith`, since the root need not begin the string.
# Two of the three sites wrap the path in a `${{ ... }}` per-OS ternary, so the root sits mid-string.
# Requiring it immediately before the tool dir is the actual invariant either way.
deny contains msg if {
	some entry in windows_tool_path
	not contains(entry.value, sprintf("%s\\http-busybox", [mise_installs_dir]))
	msg := sprintf(
		"%s: the http-busybox path must sit under the MISE_INSTALLS_DIR install-mise exports (%q)",
		[entry.path, mise_installs_dir],
	)
}
