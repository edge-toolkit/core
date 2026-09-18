# Unit tests for the trailing-backslash rule, run by `conftest verify`.
#
# They exist because the rule is one regex over every string leaf of a parsed document, and both halves of that
# can fail open silently: a regex that stops matching, or a `walk` that stops descending. Either leaves every
# config passing while nothing is examined, which is indistinguishable from a clean repo when the only evidence
# is a zero-denial run over files that are already correct.
package no_trailing_backslash_test

import data.no_trailing_backslash

parsed(contents) := [{"path": ".mise/config.toml", "contents": contents}]

# A continuation is a backslash with the line's newline immediately after it.
test_backslash_before_newline_is_flagged if {
	msgs := no_trailing_backslash.deny with input as parsed({"tasks": {"build": {"run": "cargo build \\\n  --release"}}})
	count(msgs) == 1
}

# The rule is only worth anything if `walk` reaches a leaf buried under maps and arrays.
# That is where every real occurrence lives -- a task body nested under [tasks.<name>], or one entry of a list.
test_deeply_nested_leaf_is_reached if {
	nested := {"a": {"b": [{"c": ["ok", "cmd \\\n  more"]}]}}
	msgs := no_trailing_backslash.deny with input as parsed(nested)
	count(msgs) == 1
}

# A multi-line body without continuations is the normal case and must stay quiet.
test_plain_newline_is_not_a_continuation if {
	msgs := no_trailing_backslash.deny with input as parsed({"run": "set -euo pipefail\ncargo build"})
	count(msgs) == 0
}

# A backslash inside a value is not a continuation: Windows paths and regex escapes are both legitimate.
test_backslash_inside_a_value_is_not_a_continuation if {
	msgs := no_trailing_backslash.deny with input as parsed({"env": {"DIR": "C:\\Users\\runner", "RE": "\\d+"}})
	count(msgs) == 0
}

# A backslash as the last character of the whole value has no newline after it, so it joins nothing.
test_backslash_at_end_of_value_is_not_a_continuation if {
	msgs := no_trailing_backslash.deny with input as parsed({"env": {"DIR": "C:\\Users\\runner\\"}})
	count(msgs) == 0
}
