# Unit tests for the CHECKS.md freshness policy, run by `conftest verify`.
#
# The real task only ever sees today's document, so the clock is pinned here to reach every boundary.
package checks_md_test

import data.checks_md

lines(texts) := [{"Kind": "Path", "Original": text, "Value": text} | some text in texts]

document(header) := [{"path": "CHECKS.md", "contents": lines(array.concat(["# Checks", ""], header))}]

stamp(date) := $"- **Generated:** {date}"

# Noon UTC on 2026-10-27, so a whole-day boundary is never within an hour of the clock.
now := time.parse_rfc3339_ns("2026-10-27T12:00:00Z")

reports(msgs, fragment) if {
	some msg in msgs
	contains(msg, fragment)
}

test_a_document_generated_today_is_accepted if {
	count(checks_md.deny) == 0 with input as document([stamp("2026-10-27")]) with time.now_ns as now
}

test_a_document_exactly_seven_days_old_is_accepted if {
	count(checks_md.deny) == 0 with input as document([stamp("2026-10-20")]) with time.now_ns as now
}

test_a_document_eight_days_old_is_rejected if {
	reports(checks_md.deny, "more than 7 days ago") with input as document([stamp("2026-10-19")])
		with time.now_ns as now
}

test_a_document_dated_tomorrow_is_rejected if {
	reports(checks_md.deny, "in the future") with input as document([stamp("2026-10-28")]) with time.now_ns as now
}

test_a_document_without_a_stamp_is_rejected if {
	reports(checks_md.deny, "no `- **Generated:**") with input as document(["- **Checks:** 75"]) with time.now_ns as now
}

test_a_stamp_with_trailing_text_is_not_a_stamp if {
	reports(checks_md.deny, "no `- **Generated:**") with input as document([$"{stamp("2026-10-27")} by hand"])
		with time.now_ns as now
}

test_two_different_stamps_are_rejected if {
	header := [stamp("2026-10-27"), stamp("2026-10-26")]
	reports(checks_md.deny, "2 `- **Generated:**` lines") with input as document(header) with time.now_ns as now
}

test_two_identical_stamps_are_rejected if {
	header := [stamp("2026-10-27"), stamp("2026-10-27")]
	reports(checks_md.deny, "2 `- **Generated:**` lines") with input as document(header) with time.now_ns as now
}

test_a_stamp_that_is_not_a_calendar_date_is_rejected if {
	reports(checks_md.deny, "not a calendar date") with input as document([stamp("2026-02-30")])
		with time.now_ns as now
}
