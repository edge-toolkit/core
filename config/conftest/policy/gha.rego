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
