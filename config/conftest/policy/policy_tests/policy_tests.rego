# Every conftest policy carries an accompanying `<name>_test.rego`, or an entry saying why it does not.
# Run with `--namespace policy_tests` over the policy tree itself, parsed with `ignore` so each file arrives as
# a path; only the paths are read here, never the contents.
#
# A policy rule that stops matching does not report anything -- it reports nothing, which is the same output as
# a clean repo. The real `conftest-check-*` tasks only ever run over files that already comply, so they cannot
# tell those two apart, and a rule can sit dead for months looking like a passing check. Feeding a known-bad
# input is the only thing that distinguishes them, and a test file is where that input lives.
package policy_tests

# conftest reports native separators when it walks a directory, so a Windows lane sees backslashes.
normalised(path) := replace(path, "\\", "/")

rego_files contains path if {
	some file in input
	path := normalised(file.path)
	endswith(path, ".rego")
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

# Policies deliberately left without tests, each with the reason.
#
# The first group is exempt for good: their matching is direct -- a named key read and compared against another
# named key -- so a break is a rename, which turns the check red or shows up in the diff that caused it. There is
# no silent-failure mode for synthetic input to catch, and a test would only restate the rule.
#
# The second group is not a judgement, it is debt. These are pattern-driven in the same way as the policies that
# do carry tests, and the same silent-failure mode applies to them; they simply have not been written yet. Delete
# the entry as each one gets its test rather than letting the group settle in.
untested_policy := {
	"config/conftest/policy/cross/wasm-bindgen-sync.rego": "one equality between two named values",
	"config/conftest/policy/gha_action/gha_action.rego": "reads named keys off one action file, no matching",
	"config/conftest/policy/jscpd/jscpd.rego": "two arithmetic comparisons against one declared number",
	"config/conftest/policy/pyproject/pyproject.rego": "direct key presence checks over one table",
	"config/conftest/policy/dockerfile/dockerfile.rego": "pattern-driven; predates the rule, write tests",
	"config/conftest/policy/gha/gha.rego": "pattern-driven; predates the rule, write tests",
	"config/conftest/policy/gha_combined/gha_combined.rego": "pattern-driven; predates the rule, write tests",
	"config/conftest/policy/gha_mise/gha_mise.rego": "pattern-driven; predates the rule, write tests",
	"config/conftest/policy/gha_uses/gha_uses.rego": "pattern-driven; predates the rule, write tests",
}

deny contains msg if {
	some path in policy_files
	not untested_policy[path]
	expected := expected_test(path)
	not expected in paired_tests
	msg := sprintf("%s: add %s, or record the policy in untested_policy with its reason", [path, expected])
}

# The map is a two-way contract, the same way a lint suppression is.
# An entry kept past the test that answers it reads as a standing decision not to test a policy that is in fact
# tested, so writing the test has to be what removes the entry.
deny contains msg if {
	some path, reason in untested_policy
	expected_test(path) in paired_tests
	msg := sprintf("%s: now has a test, so drop its untested_policy entry (%s)", [path, reason])
}

# An entry naming a policy that is no longer there has outlived what it described.
deny contains msg if {
	some path, reason in untested_policy
	not path in policy_files
	msg := sprintf("%s: untested_policy names a policy that no longer exists (%s)", [path, reason])
}
