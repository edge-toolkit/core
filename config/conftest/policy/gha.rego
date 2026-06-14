# GitHub Actions workflow policy, evaluated per file: conftest reads each .yml
# independently (no --combine, since there are no cross-file YAML rules).
# Replicates the gha-* ast-grep rules; running both is fine. Selected with
# `--namespace gha`, so it only runs against workflow YAML, never the TOML inputs.
package gha

# Every workflow must set the default run shell to bash, so steps run in bash on
# every runner (Windows included) without a per-step `shell:`.
deny contains msg if {
	not input.defaults.run.shell == "bash"
	msg := "workflow must set defaults.run.shell: bash"
}

# Steps must not override the shell -- rely on the workflow default (write the
# step in bash rather than switching to PowerShell on Windows runners).
deny contains msg if {
	some name, job in input.jobs
	some step in job.steps
	step.shell
	msg := sprintf("job %q sets shell: on a step; use the workflow default (bash)", [name])
}
