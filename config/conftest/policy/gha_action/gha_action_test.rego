# Unit tests for the composite-action policy, run by `conftest verify`.
#
# Every action in the repo already declares the full shell and the upload guard, so the real task cannot tell a
# passing action from a rule that no longer matches.
package gha_action_test

import data.gha_action

full_shell := "bash --noprofile --norc -euo pipefail {0}"

composite(step) := {"runs": {"using": "composite", "steps": [step]}}

wrong_shell(msgs) if {
	some msg in msgs
	contains(msg, "composite-action steps must use shell")
}

unguarded_upload(msgs) if {
	some msg in msgs
	contains(msg, "must set with.if-no-files-found: error")
}

test_a_run_step_with_the_full_shell_is_accepted if {
	not wrong_shell(gha_action.deny) with input as composite({"name": "s", "run": "true", "shell": full_shell})
}

test_the_bash_shorthand_is_denied if {
	wrong_shell(gha_action.deny) with input as composite({"name": "s", "run": "true", "shell": "bash"})
}

test_a_run_step_with_no_shell_is_denied if {
	wrong_shell(gha_action.deny) with input as composite({"name": "s", "run": "true"})
}

test_a_uses_step_needs_no_shell if {
	not wrong_shell(gha_action.deny) with input as composite({"name": "s", "uses": "actions/checkout@v5"})
}

test_a_non_composite_action_is_not_checked if {
	action := {"runs": {"using": "node24", "steps": [{"name": "s", "run": "true", "shell": "bash"}]}}
	not wrong_shell(gha_action.deny) with input as action
}

test_an_upload_without_the_guard_is_denied if {
	unguarded_upload(gha_action.deny) with input as composite({"name": "u", "uses": "actions/upload-artifact@v5"})
}

test_an_upload_that_only_warns_is_denied if {
	step := {"name": "u", "uses": "actions/upload-artifact@v5", "with": {"if-no-files-found": "warn"}}
	unguarded_upload(gha_action.deny) with input as composite(step)
}

test_an_upload_that_errors_is_accepted if {
	step := {"name": "u", "uses": "actions/upload-artifact@v5", "with": {"if-no-files-found": "error"}}
	not unguarded_upload(gha_action.deny) with input as composite(step)
}
