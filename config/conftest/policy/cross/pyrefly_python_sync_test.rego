# Unit tests for the pyrefly python-version pin, run by `conftest verify`.
#
# The real task only ever sees a pair that already agrees, which looks the same as a rule that never matches.
package cross_test

import data.cross

combined(mise_python, pyrefly) := [
	{"path": ".mise/config.toml", "contents": {"tools": {"python": mise_python}}},
	{"path": "config/pyrefly.toml", "contents": pyrefly},
]

reports(msgs, fragment) if {
	some msg in msgs
	contains(msg, fragment)
}

test_a_matching_pin_is_accepted if {
	msgs := cross.deny with input as combined("3.13.14", {"python-version": "3.13.14"})
	not reports(msgs, "pyrefly.toml")
}

test_a_pin_on_the_object_form_is_accepted if {
	msgs := cross.deny with input as combined({"version": "3.13.14"}, {"python-version": "3.13.14"})
	not reports(msgs, "pyrefly.toml")
}

test_a_drifted_pin_is_flagged if {
	msgs := cross.deny with input as combined("3.13.14", {"python-version": "3.12.1"})
	reports(msgs, "python-version \"3.12.1\" != mise python \"3.13.14\"")
}

test_an_unset_pin_is_flagged if {
	msgs := cross.deny with input as combined("3.13.14", {})
	reports(msgs, "python-version is unset")
}
