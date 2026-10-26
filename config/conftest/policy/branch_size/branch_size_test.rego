# Unit tests for the branch size policy, run by `conftest verify`.
#
# The real task only sees the branch it runs on, which is usually small, so the limit is exercised here.
package branch_size_test

import data.branch_size

# A changed-file list of `total` distinct paths, in the shape the ignore parser hands over, with a blank line.
listing(total) := [{"path": "-", "contents": array.concat(
	[{"Kind": "Path", "Original": $"file-{n}.rs", "Value": $"file-{n}.rs"} | some n in numbers.range(1, total)],
	[{"Kind": "Path", "Original": "", "Value": ""}],
)}]

test_a_branch_at_the_limit_passes if {
	count(branch_size.deny) == 0 with input as listing(100)
}

test_a_branch_over_the_limit_is_refused if {
	msgs := branch_size.deny with input as listing(101)
	count(msgs) == 1
	some msg in msgs
	contains(msg, "modifies 101 files")
}
