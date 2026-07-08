# Cross-file check: every input passed to a LOCAL composite action must be one that the action declares.
# Run with `--namespace gha_uses` over the workflow YAML plus every .github/actions/*/action.yaml, combined
# (--combine) so a single evaluation resolves each `uses: ./.github/actions/<name>` against that action's `inputs:`.
# GitHub Actions only emits a *warning* -- never an error -- when a caller passes a `with:` key the target action
# does not declare, so a typo'd input name silently defaults to "" and the intended value is dropped. (Concrete
# regression: install-mise-tools passed `install-action:` to install-mise, whose input is `install-action-tools`,
# so co-installed tools like cargo-llvm-cov silently never installed and the coverage job failed with
# `no such command: llvm-cov`.) This promotes that class of typo to a hard failure.
package gha_uses

# action dir name -> set of declared input keys, one entry per parsed .github/actions/<name>/action.yaml.
declared_inputs[name] := names if {
	some file in input
	parts := split(file.path, "/")
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
