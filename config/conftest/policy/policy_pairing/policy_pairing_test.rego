# Unit tests for the policy/test pairing rule, run by `conftest verify`.
#
# They exist for the reason the rule itself exists: the pairing is decided by suffix matching over paths, which
# fails open. If `_test.rego` stopped being recognised, every policy would look untested and the check would go
# loudly red -- but the opposite break, a policy file that stops being recognised as one, makes the requirement
# silently apply to nothing. The real run over the real tree passes either way once the tree complies.
#
# A fixture holds a handful of paths rather than the whole policy tree, so every real `untested_policy` entry is
# absent from it and the stale-entry rule reports each one. That noise is expected here, and is why these assert
# on the message they are about rather than on a count.
package policy_pairing_test

import data.policy_pairing

files(paths) := [entry | some p in paths; entry := {"path": p, "contents": [[]]}]

names(msgs, fragment) if {
	some msg in msgs
	contains(msg, fragment)
}

demo_wanted := "add config/conftest/policy/demo/demo_test.rego"

test_policy_with_its_test_is_accepted if {
	msgs := policy_pairing.deny with input as files([
		"config/conftest/policy/demo/demo.rego",
		"config/conftest/policy/demo/demo_test.rego",
	])
	not names(msgs, demo_wanted)
}

test_policy_without_a_test_is_flagged if {
	msgs := policy_pairing.deny with input as files(["config/conftest/policy/demo/demo.rego"])
	names(msgs, demo_wanted)
}

# A test file is not itself a policy, so it must not demand a test of its own.
test_test_file_does_not_demand_its_own_test if {
	msgs := policy_pairing.deny with input as files(["config/conftest/policy/demo/demo_test.rego"])
	not names(msgs, "demo_test_test.rego")
}

# Pairing is per policy, so another policy's test does not answer for this one.
test_another_policy_test_does_not_count if {
	msgs := policy_pairing.deny with input as files([
		"config/conftest/policy/demo/demo.rego",
		"config/conftest/policy/other/other_test.rego",
	])
	names(msgs, demo_wanted)
}

# conftest reports native separators when walking a directory, which a Windows lane hands over as backslashes.
# Unnormalised, the `.rego` suffix still matches, so the policy is still demanded -- but its test is never
# recognised as the answer, and the check goes red on that lane alone for files that are perfectly paired.
test_windows_separators_still_pair_up if {
	msgs := policy_pairing.deny with input as files([
		"config\\conftest\\policy\\demo\\demo.rego",
		"config\\conftest\\policy\\demo\\demo_test.rego",
	])
	not names(msgs, demo_wanted)
}

# Anything that is not a .rego is none of this rule's business.
test_non_rego_files_are_ignored if {
	msgs := policy_pairing.deny with input as files(["config/conftest/policy/demo/README.md"])
	not names(msgs, "README")
}

# An exception excuses the policy it names, and only that one.
test_excepted_policy_needs_no_test if {
	msgs := policy_pairing.deny with input as files(["config/conftest/policy/jscpd/jscpd.rego"])
	not names(msgs, "add config/conftest/policy/jscpd/jscpd_test.rego")
}

test_exception_does_not_cover_other_policies if {
	msgs := policy_pairing.deny with input as files([
		"config/conftest/policy/jscpd/jscpd.rego",
		"config/conftest/policy/demo/demo.rego",
	])
	names(msgs, demo_wanted)
}

# Writing the test is what retires the entry, so an entry that outlives its answer reports.
test_excepted_policy_that_gained_a_test_reports_the_stale_entry if {
	msgs := policy_pairing.deny with input as files([
		"config/conftest/policy/jscpd/jscpd.rego",
		"config/conftest/policy/jscpd/jscpd_test.rego",
	])
	names(msgs, "drop its untested_policy entry")
}

test_excepted_policy_that_was_deleted_reports_the_stale_entry if {
	msgs := policy_pairing.deny with input as files(["config/conftest/policy/demo/demo.rego"])
	names(msgs, "names a policy that no longer exists")
}
