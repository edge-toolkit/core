# CHECKS.md's generation date, held to at most a week old instead of re-rendering the document on every change.
# Run with `--namespace checks_md` over CHECKS.md, `--combine`d and parsed with `ignore`, which hands the file over as
# its lines; the `Original` field of each is the line's text.
package checks_md

# How many whole days may pass since generation before the document has to be regenerated.
max_age_days := 7

ns_per_day := ((24 * 60) * 60) * 1000000000

# The whole header line the generator writes, capturing its date.
stamp_pattern := `^- \*\*Generated:\*\* ([0-9]{4}-[0-9]{2}-[0-9]{2})$`

# The date on each `- **Generated:** YYYY-MM-DD` line of CHECKS.md, one entry per line, so repeats are counted.
stamps := [match[1] |
	some file in input
	endswith(file.path, "CHECKS.md")
	some line in file.contents
	some match in regex.find_all_string_submatch_n(stamp_pattern, line.Original, 1)
]

# Midnight UTC on `date`, in nanoseconds since the epoch; undefined when `date` is not a calendar date.
midnight_ns(date) := time.parse_ns("2006-01-02", date)

# Whole days from midnight UTC on `date` to now; meaningful only for a date not in the future.
age_days(date) := floor((time.now_ns() - midnight_ns(date)) / ns_per_day)

# CHECKS.md must say when it was generated, which only the generator writes.
deny contains msg if {
	count(stamps) == 0
	msg := "CHECKS.md: no `- **Generated:** YYYY-MM-DD` line; regenerate it"
}

# A document stamped more than once was not written by the generator, which stamps it exactly once.
deny contains msg if {
	count(stamps) > 1
	msg := $"CHECKS.md: {count(stamps)} `- **Generated:**` lines; regenerate it"
}

# A stamp the shape allows but the calendar does not, such as 2026-02-30, was written by hand and has no age.
deny contains msg if {
	some date in stamps
	not midnight_ns(date)
	msg := $"CHECKS.md: generated {date}, which is not a calendar date; regenerate it"
}

# The document is regenerated at least once every `max_age_days` days.
deny contains msg if {
	some date in stamps
	age_days(date) > max_age_days
	msg := $"CHECKS.md: generated {date}, more than {max_age_days} days ago; regenerate it"
}

# The generator stamps today's UTC date, so a later one was written by hand.
deny contains msg if {
	some date in stamps
	midnight_ns(date) > time.now_ns()
	msg := $"CHECKS.md: generated {date}, which is in the future; regenerate it"
}
