# Unit tests for the checks badge policy, run by `conftest verify`.
#
# The real task only sees a README already in step, which looks the same as a rule that never matches.
package checks_badge_test

import data.checks_badge

lines(texts) := [{"Kind": "Path", "Original": text, "Value": text} | some text in texts]

combined(rules_line, badge_line) := [
	{"path": "CHECKS.md", "contents": lines(["# Checks", "", rules_line])},
	{"path": "README.md", "contents": lines(["# edge-toolkit core", badge_line])},
]

badge(recorded) := $"[checks-badge]: https://img.shields.io/badge/checks-%3E%3D{recorded}-blue"

rules(total) := $"- **Rules:** {total} custom local rules, or non-default strict linter settings"

reports(msgs, fragment) if {
	some msg in msgs
	contains(msg, fragment)
}

test_a_badge_at_the_rounded_down_count_is_accepted if {
	count(checks_badge.deny) == 0 with input as combined(rules(339), badge(320))
}

test_a_badge_on_an_exact_step_is_accepted if {
	count(checks_badge.deny) == 0 with input as combined(rules(340), badge(340))
}

test_a_badge_left_behind_by_a_step_is_flagged if {
	msgs := checks_badge.deny with input as combined(rules(341), badge(320))
	reports(msgs, "needs `[checks-badge]: https://img.shields.io/badge/checks-%3E%3D340-blue`")
}

test_a_badge_ahead_of_the_count_is_flagged if {
	msgs := checks_badge.deny with input as combined(rules(319), badge(320))
	reports(msgs, "%3E%3D300-blue")
}

test_a_missing_badge_is_flagged if {
	msgs := checks_badge.deny with input as combined(rules(320), "no badge here")
	reports(msgs, "no `[checks-badge]: ` line")
}

test_a_missing_rules_count_is_flagged if {
	msgs := checks_badge.deny with input as combined("- **Checks:** 69", badge(320))
	reports(msgs, "no `- **Rules:** N` line")
}
