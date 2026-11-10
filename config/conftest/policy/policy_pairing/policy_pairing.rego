# Every conftest policy carries an accompanying `<name>_test.rego`, or an entry saying why it does not.
# Run with `--namespace policy_pairing` over the policy tree itself, parsed with `ignore` so each file arrives
# as a path; only the paths are read here, never the contents.
#
# A policy rule that stops matching does not report anything -- it reports nothing, which is the same output as
# a clean repo. The real `conftest-check-*` tasks only ever run over files that already comply, so they cannot
# tell those two apart, and a rule can sit dead for months looking like a passing check. Feeding a known-bad
# input is the only thing that distinguishes them, and a test file is where that input lives.

# METADATA
# custom:
#   meta: true
package policy_pairing

import data.paths

rego_files contains file.path if {
	some file in paths.files
	endswith(file.path, ".rego")
}

# Named away from a `test_` prefix: conftest verify treats any rule so named as a unit test to execute.
paired_tests contains path if {
	some path in rego_files
	endswith(path, "_test.rego")
}

policy_files contains path if {
	some path in rego_files
	not endswith(path, "_test.rego")
}

expected_test(path) := sprintf("%s_test.rego", [trim_suffix(path, ".rego")])

# Policies deliberately left without tests, as path -> the reason it is safe to leave that one untested.
# Built through `object.union` so its type is a string-keyed object even while it holds no entries: a bare `{}`
# types as an object with no keys at all, and the compiler then rejects every lookup below as an undefined ref.
untested_policy := object.union({}, {})

# Every policy must have a test file beside it, or a reasoned untested_policy entry.
deny contains msg if {
	some path in policy_files
	not untested_policy[path]
	expected := expected_test(path)
	not expected in paired_tests
	msg := $"{path}: add {expected}, or record the policy in untested_policy with its reason"
}

# The map is a two-way contract, the same way a lint suppression is.
# An entry kept past the test that answers it reads as a standing decision not to test a policy that is in fact
# tested, so writing the test has to be what removes the entry.
deny contains msg if {
	some path, reason in untested_policy
	expected_test(path) in paired_tests
	msg := $"{path}: now has a test, so drop its untested_policy entry ({reason})"
}

# An entry naming a policy that is no longer there has outlived what it described.
deny contains msg if {
	some path, reason in untested_policy
	not path in policy_files
	msg := $"{path}: untested_policy names a policy that no longer exists ({reason})"
}
