# Unit tests for the advisory review-date rule, run by `conftest verify`.
#
# The lapse rule reads the wall clock, which makes it the one policy here whose result changes on its own: a
# real run passes today and fails after the date without anything in the repo changing. So every case pins
# `time.now_ns` with `with`, and the dates in the fixtures are chosen relative to that pinned instant rather
# than to today -- otherwise the tests would themselves expire.
package advisories_test

import data.advisories

# 2026-10-15T00:00:00Z in nanoseconds, the instant the fixtures are written around.
review_day := 1792022400000000000

# One day either side, for the boundary cases.
day_ns := 86400000000000

deny_file(entries) := [{"path": "config/deny.toml", "contents": {"advisories": {"ignore": entries}}}]

dated(id, date) := {"id": id, "reason": sprintf("expires %s", [date])}

names(msgs, fragment) if {
	some msg in msgs
	contains(msg, fragment)
}

test_entry_still_inside_its_review_date_passes if {
	msgs := advisories.deny with input as deny_file([dated("RUSTSEC-2026-0285", "2026-10-15")])
		with time.now_ns as review_day
	count(msgs) == 0
}

# The entry survives the whole of its review date, so the last second of that day is still fine.
test_entry_passes_until_the_end_of_its_review_date if {
	msgs := advisories.deny with input as deny_file([dated("RUSTSEC-2026-0285", "2026-10-15")])
		with time.now_ns as (review_day + day_ns) - 1
	count(msgs) == 0
}

test_entry_lapses_the_day_after if {
	msgs := advisories.deny with input as deny_file([dated("RUSTSEC-2026-0285", "2026-10-15")])
		with time.now_ns as review_day + day_ns
	names(msgs, "lapsed on 2026-10-15")
}

test_bare_id_without_a_review_date_is_flagged if {
	msgs := advisories.deny with input as deny_file(["RUSTSEC-2026-0285"])
		with time.now_ns as review_day
	names(msgs, "has no")
}

# A reason that says something else is not a date, and must not be read as one.
test_entry_whose_reason_carries_no_date_is_flagged if {
	entry := {"id": "RUSTSEC-2026-0285", "reason": "upstream has not released a fix"}
	msgs := advisories.deny with input as deny_file([entry]) with time.now_ns as review_day
	names(msgs, "has no")
}

# Anchored, so trailing prose cannot smuggle a date past the check.
test_entry_with_prose_after_the_date_is_flagged if {
	entry := {"id": "RUSTSEC-2026-0285", "reason": "expires 2026-10-15 unless upstream moves"}
	msgs := advisories.deny with input as deny_file([entry]) with time.now_ns as review_day
	names(msgs, "has no")
}

test_each_lapsed_entry_is_reported_separately if {
	entries := [dated("RUSTSEC-2026-0285", "2026-10-15"), dated("GHSA-3w8q-xq97-5j7x", "2026-10-15")]
	msgs := advisories.deny with input as deny_file(entries) with time.now_ns as review_day + day_ns
	names(msgs, "RUSTSEC-2026-0285")
	names(msgs, "GHSA-3w8q-xq97-5j7x")
}

# Only deny.toml carries this table; another TOML file in the combined input is none of this rule's business.
test_other_files_in_the_combined_input_are_ignored if {
	other := [{"path": "Cargo.toml", "contents": {"advisories": {"ignore": ["RUSTSEC-2026-0285"]}}}]
	msgs := advisories.deny with input as other with time.now_ns as review_day
	count(msgs) == 0
}
