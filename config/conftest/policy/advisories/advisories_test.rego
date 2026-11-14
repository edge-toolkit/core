# Unit tests for the advisory review-date rules, run by `conftest verify`.
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

dated(id, date) := {"id": id, "reason": $"expires {date}"}

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

test_entries_sharing_one_date_pass if {
	entries := [dated("RUSTSEC-2026-0285", "2026-10-15"), dated("GHSA-3w8q-xq97-5j7x", "2026-10-15")]
	msgs := advisories.deny with input as deny_file(entries) with time.now_ns as review_day
	count(msgs) == 0
}

# A second date is flagged even while both are still in the future.
test_entries_with_different_dates_are_flagged if {
	entries := [dated("RUSTSEC-2026-0285", "2026-10-15"), dated("GHSA-3w8q-xq97-5j7x", "2026-11-30")]
	msgs := advisories.deny with input as deny_file(entries) with time.now_ns as review_day
	count(msgs) == 1
	names(msgs, "expire on 2 dates")
}

# Only deny.toml carries this table; another TOML file in the combined input is none of this rule's business.
test_other_files_in_the_combined_input_are_ignored if {
	other := [{"path": "Cargo.toml", "contents": {"advisories": {"ignore": ["RUSTSEC-2026-0285"]}}}]
	msgs := advisories.deny with input as other with time.now_ns as review_day
	count(msgs) == 0
}

# deny.toml plus the tools-scan ignore file, the pair the combined run feeds the policy.
with_tools(tool_entries) := array.concat(
	deny_file([dated("RUSTSEC-2026-0285", "2026-10-15")]),
	[{"path": "config/trivyignore-tools.yaml", "contents": {"vulnerabilities": tool_entries}}],
)

test_tool_entry_on_the_deny_date_passes if {
	entry := {"id": "CVE-2026-63209", "paths": ["aqua-k3d-io-k3d/5.9.0/k3d"], "expired_at": "2026-10-15"}
	msgs := advisories.deny with input as with_tools([entry]) with time.now_ns as review_day
		with advisories.old_advisories as set()
	count(msgs) == 0
}

# A later date is still drift: it would leave the entry standing past the milestone review.
test_tool_entry_on_another_date_is_flagged if {
	entry := {"id": "CVE-2026-63209", "expired_at": "2026-11-30"}
	msgs := advisories.deny with input as with_tools([entry]) with time.now_ns as review_day
		with advisories.old_advisories as set()
	count(msgs) == 1
	names(msgs, "CVE-2026-63209")
	names(msgs, "expires on 2026-11-30")
}

test_tool_entry_without_a_date_is_flagged if {
	msgs := advisories.deny with input as with_tools([{"id": "CVE-2026-63209"}]) with time.now_ns as review_day
		with advisories.old_advisories as set()
	count(msgs) == 1
	names(msgs, "has no `expired_at`")
}

test_each_drifting_tool_entry_is_reported_separately if {
	entries := [{"id": "CVE-2026-63209", "expired_at": "2027-01-01"}, {"id": "CVE-2026-78661"}]
	msgs := advisories.deny with input as with_tools(entries) with time.now_ns as review_day
		with advisories.old_advisories as set()
	count(msgs) == 2
	names(msgs, "CVE-2026-63209")
	names(msgs, "CVE-2026-78661")
}

# The tools file is read only at its own path; a same-shaped file elsewhere is not an ignore list.
test_vulnerabilities_in_another_file_are_ignored if {
	other := [{"path": "config/trivyignore.yaml", "contents": {"vulnerabilities": [{"id": "CVE-2026-63209"}]}}]
	msgs := advisories.deny with input as array.concat(deny_file([dated("RUSTSEC-2026-0285", "2026-10-15")]), other)
		with time.now_ns as review_day
	count(msgs) == 0
}

with_misconfigs(misconfig_entries) := array.concat(
	deny_file([dated("RUSTSEC-2026-0285", "2026-10-15")]),
	[{"path": "config/trivyignore.yaml", "contents": {"misconfigurations": misconfig_entries}}],
)

test_misconfiguration_on_the_deny_date_passes if {
	entry := {"id": "DS-0026", "paths": ["Dockerfile"], "expired_at": "2026-10-15"}
	msgs := advisories.deny with input as with_misconfigs([entry]) with time.now_ns as review_day
	count(msgs) == 0
}

test_misconfiguration_on_another_date_is_flagged if {
	msgs := advisories.deny with input as with_misconfigs([{"id": "DS-0026", "expired_at": "2027-06-30"}])
		with time.now_ns as review_day
	count(msgs) == 1
	names(msgs, "config/trivyignore.yaml: accepted finding \"DS-0026\" expires on 2027-06-30")
}

test_misconfiguration_without_a_date_is_flagged if {
	msgs := advisories.deny with input as with_misconfigs([{"id": "KSV-0013"}]) with time.now_ns as review_day
	count(msgs) == 1
	names(msgs, "config/trivyignore.yaml: accepted finding \"KSV-0013\" has no `expired_at`")
}

# A misconfiguration id is a rule code, not an advisory, so the age allowlist never reads it.
test_misconfiguration_ids_are_not_advisories if {
	msgs := advisories.deny with input as with_misconfigs([{"id": "KSV-2023-0001", "expired_at": "2026-10-15"}])
		with time.now_ns as review_day
		with advisories.old_advisories as set()
	count(msgs) == 0
}

tool_on_review_day(id) := {"id": id, "expired_at": "2026-10-15"}

# Every year-numbered id scheme the scanners report, each just before and just at the cutoff year.
dated_ids_before_cutoff := {
	"CVE-2025-1302",
	"RUSTSEC-2025-0134",
	"PYSEC-2024-110",
	"GO-2023-2102",
	"MAL-2025-41",
	"OSV-2022-893",
}

dated_ids_from_cutoff := {"CVE-2026-63209", "RUSTSEC-2026-0285", "PYSEC-2026-7", "GO-2026-4001", "MAL-2026-1"}

# GHSA ids are base-32 segments with no year; four-digit first segments included, none of them may read as one.
undated_ids := {"GHSA-hrxh-6v49-42gf", "GHSA-2222-3333-4444", "GHSA-9999-xxxx-q2c4"}

test_every_dated_prefix_before_the_cutoff_is_flagged_in_the_tools_file if {
	every id in dated_ids_before_cutoff {
		msgs := advisories.deny with input as with_tools([tool_on_review_day(id)])
			with time.now_ns as review_day
			with advisories.old_advisories as set()
		names(msgs, $"config/trivyignore-tools.yaml: accepted advisory \"{id}\" is from")
	}
}

test_every_dated_prefix_before_the_cutoff_is_flagged_in_deny_toml if {
	every id in dated_ids_before_cutoff {
		msgs := advisories.deny with input as deny_file([dated(id, "2026-10-15")])
			with time.now_ns as review_day
			with advisories.old_advisories as set()
		names(msgs, $"config/deny.toml: accepted advisory \"{id}\" is from")
	}
}

# A bare id string in deny.toml is still an accepted advisory, and still dated by its prefix.
test_bare_old_id_in_deny_toml_is_flagged if {
	msgs := advisories.deny with input as deny_file(["RUSTSEC-2023-0071"])
		with time.now_ns as review_day
		with advisories.old_advisories as set()
	names(msgs, "\"RUSTSEC-2023-0071\" is from 2023")
}

test_every_dated_prefix_before_the_cutoff_passes_once_allowlisted if {
	every id in dated_ids_before_cutoff {
		msgs := advisories.deny with input as with_tools([tool_on_review_day(id)])
			with time.now_ns as review_day
			with advisories.old_advisories as {id}
		count(msgs) == 0
	}
}

# Ids from the cutoff year on, and GHSA ids that carry no year at all, need no allowlist entry.
test_ids_from_the_cutoff_or_without_a_year_pass_unlisted if {
	every id in dated_ids_from_cutoff | undated_ids {
		msgs := advisories.deny with input as with_tools([tool_on_review_day(id)])
			with time.now_ns as review_day
			with advisories.old_advisories as set()
		count(msgs) == 0
	}
}

test_advisory_year_reads_each_prefix if {
	advisories.advisory_year("CVE-2025-1302") == 2025
	advisories.advisory_year("RUSTSEC-2023-0071") == 2023
	advisories.advisory_year("PYSEC-2024-110") == 2024
	advisories.advisory_year("GO-2026-4001") == 2026
	not advisories.advisory_year("GHSA-2222-3333-4444")
	not advisories.advisory_year("CVE-2025")
}

test_unlisted_old_advisory_in_the_tools_file_is_flagged if {
	msgs := advisories.deny with input as with_tools([tool_on_review_day("CVE-2025-99999")])
		with time.now_ns as review_day
		with advisories.old_advisories as set()
	count(msgs) == 1
	names(msgs, "\"CVE-2025-99999\" is from 2025")
}

test_unlisted_old_advisory_in_deny_toml_is_flagged if {
	msgs := advisories.deny with input as deny_file([dated("RUSTSEC-2024-9999", "2026-10-15")])
		with time.now_ns as review_day
		with advisories.old_advisories as set()
	count(msgs) == 1
	names(msgs, "config/deny.toml: accepted advisory \"RUSTSEC-2024-9999\" is from 2024")
}

test_listed_old_advisory_passes if {
	msgs := advisories.deny with input as with_tools([tool_on_review_day("CVE-2023-12345")])
		with time.now_ns as review_day
		with advisories.old_advisories as {"CVE-2023-12345"}
	count(msgs) == 0
}

# A GHSA id carries no year, so the age rule has nothing to read and leaves it alone.
test_advisory_without_a_year_is_not_judged_by_age if {
	msgs := advisories.deny with input as with_tools([tool_on_review_day("GHSA-hrxh-6v49-42gf")])
		with time.now_ns as review_day
		with advisories.old_advisories as set()
	count(msgs) == 0
}

test_allowlisted_id_no_ignore_list_accepts_is_flagged if {
	msgs := advisories.deny with input as with_tools([]) with time.now_ns as review_day
		with advisories.old_advisories as {"CVE-2024-21534"}
	count(msgs) == 1
	names(msgs, "\"CVE-2024-21534\", which no ignore list accepts")
}

# Fed deny.toml alone, a run has not read the tools file, so it cannot call an id there gone.
test_stale_rule_waits_for_every_ignore_list if {
	msgs := advisories.deny with input as deny_file([dated("RUSTSEC-2026-0285", "2026-10-15")])
		with time.now_ns as review_day
		with advisories.old_advisories as {"CVE-2024-21534"}
	count(msgs) == 0
}
