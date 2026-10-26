# Unit tests for the ast-grep rule severity policy, run by `conftest verify`.
package ast_grep_rules_test

import data.ast_grep_rules

rule(id, severity) := {"id": id, "language": "Rust", "severity": severity}

test_error_severity_passes if {
	count(ast_grep_rules.deny) == 0 with input as [{"path": "rules/a.yaml", "contents": rule("a", "error")}]
}

test_warning_severity_is_denied if {
	msgs := ast_grep_rules.deny with input as [{"path": "rules/a.yaml", "contents": rule("a", "warning")}]
	msgs == {sprintf(ast_grep_rules.severity_msg, ["rules/a.yaml", "a", "warning"])}
}

test_missing_severity_is_denied if {
	msgs := ast_grep_rules.deny with input as [{"path": "rules/a.yaml", "contents": {"id": "a", "language": "Rust"}}]
	count(msgs) == 1
	some msg in msgs
	contains(msg, "\"<unset>\"")
}

test_each_rule_of_a_multi_document_file_is_checked if {
	contents := [rule("a", "error"), rule("b", "off")]
	msgs := ast_grep_rules.deny with input as [{"path": "rules/ab.yaml", "contents": contents}]
	count(msgs) == 1
	some msg in msgs
	contains(msg, "\"b\" has severity \"off\"")
}
