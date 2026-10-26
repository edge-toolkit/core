# Every ast-grep rule under config/ast-grep/rules declares `severity: error`.
# Run with `--combine --namespace ast_grep_rules` over that directory, so each rule file arrives as one
# `{path, contents}` entry, and a multi-document file's contents as the array of its rules.
#
# Each rule here is a gate. A `warning`, `info` or `hint` reads as advisory to whoever opens the rule or meets its
# finding in an editor, and `off` disables the rule while it still sits in the ruleset looking enforced. The check
# lives here rather than in ast-grep because an ast-grep scan never reports a finding in its own rule directory.

# METADATA
# custom:
#   meta: true
package ast_grep_rules

severity_msg := "%s: ast-grep rule %q has severity %q; every rule here is a gate, so it declares `severity: error`"

# Every rule in the input, paired with the file defining it.
rules contains [file.path, rule] if {
	some file in input
	is_object(file.contents)
	rule := file.contents
}

rules contains [file.path, rule] if {
	some file in input
	is_array(file.contents)
	some rule in file.contents
}

# A rule whose severity is anything but `error`, or is missing, is not a gate.
deny contains msg if {
	some [path, rule] in rules
	severity := object.get(rule, "severity", "<unset>")
	severity != "error"
	msg := sprintf(severity_msg, [path, object.get(rule, "id", "<no id>"), severity])
}
