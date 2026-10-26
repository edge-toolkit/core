# Unit tests for the no_exemptions policy, run by `conftest verify`.
#
# The real check only ever sees rule files that already comply, so each deny is fed a known-bad input here to show
# it still fires.
package no_exemptions_test

import data.no_exemptions

guard_rule := {
	"id": "no-suppressing-unexemptable-rules",
	"pattern-either": [{"pattern-regex": "nose[m]:[^\\n]*no-non-ascii"}],
}

guard_file := {"path": "config/semgrep/guard.yaml", "contents": {"rules": [guard_rule]}}

semgrep_ascii(rule) := {"path": "config/semgrep/no-non-ascii.yaml", "contents": {"rules": [rule]}}

ast_grep_ascii(contents) := {"path": "config/ast-grep/rules/no-non-ascii.yaml", "contents": contents}

# Rule files carrying no scoping at all, and a scoped rule that is not on the list.
clean := [
	guard_file,
	semgrep_ascii({"id": "no-non-ascii", "pattern-regex": "x"}),
	ast_grep_ascii({"id": "no-non-ascii", "rule": {"kind": "string_literal"}}),
	{"path": "config/semgrep/no-todo.yaml", "contents": {"rules": [{"id": "no-todo", "paths": {"exclude": ["/x"]}}]}},
]

test_rules_without_exemptions_pass if {
	count(no_exemptions.deny) == 0 with input as clean
}

test_a_semgrep_path_exclusion_is_refused if {
	scoped := [guard_file, semgrep_ascii({"id": "no-non-ascii", "paths": {"exclude": ["/generated/**"]}})]
	messages := no_exemptions.deny with input as scoped
	count(messages) == 1
	contains(messages[_], "may not set \"paths\"")
}

test_ast_grep_files_and_ignores_are_refused if {
	scoped := [guard_file, ast_grep_ascii({"id": "no-non-ascii", "files": ["src/**"], "ignores": ["generated/**"]})]
	messages := no_exemptions.deny with input as scoped
	count(messages) == 2
	contains(messages[_], "may not set \"files\"")
	contains(messages[_], "may not set \"ignores\"")
}

test_a_guard_that_names_no_listed_rule_is_refused if {
	blind := {"path": "config/semgrep/guard.yaml", "contents": {"rules": [{
		"id": "no-suppressing-unexemptable-rules",
		"pattern-regex": "nose[m]:[^\\n]*no-todo",
	}]}}
	messages := no_exemptions.deny with input as [blind]
	count(messages) == 1
	contains(messages[_], "none of its patterns names that rule")
}

# Naming the rule only in the message is not detecting it.
test_a_guard_naming_a_rule_only_in_its_message_is_refused if {
	talker := {"path": "config/semgrep/guard.yaml", "contents": {"rules": [{
		"id": "no-suppressing-unexemptable-rules",
		"message": "Flags suppressions of no-non-ascii.",
		"pattern-either": [{"pattern-regex": "nose[m]:[^\\n]*no-todo"}],
	}]}}
	messages := no_exemptions.deny with input as [talker]
	count(messages) == 1
	contains(messages[_], "none of its patterns names that rule")
}

test_a_missing_guard_is_refused if {
	messages := no_exemptions.deny with input as [semgrep_ascii({"id": "no-non-ascii"})]
	count(messages) == 1
	contains(messages[_], "flags inline suppressions of the unexemptable rules")
}
