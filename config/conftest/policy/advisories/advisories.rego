# Review dates on the advisory ignores in config/deny.toml, evaluated from the combined TOML input.
# Run with `--namespace advisories` by conftest-check-toml, which feeds deny.toml alongside the other config
# TOML, so `input` is the --combine array of {path, contents} rather than one file's contents.
#
# Every entry in `[advisories].ignore` is an accepted finding, and an accepted finding that nobody looks at
# again is indistinguishable from one nobody noticed. So each carries a review date in its `reason`, and this
# fails the build once that date passes -- the entry then has to be dropped, fixed, or consciously re-dated,
# which is a reviewable one-line diff rather than silence.
#
# The undated rule is what keeps that true of future entries: a new ignore added as a bare id, or with a
# reason this cannot parse, fails immediately rather than quietly becoming the one exception with no deadline.
package advisories

# The `reason` spelling the date is read from, anchored so trailing prose cannot hide a second date.
expiry_pattern := `^expires ([0-9]{4}-[0-9]{2}-[0-9]{2})$`

undated_msg := "config/deny.toml: advisory ignore %q has no `reason = \"expires YYYY-MM-DD\"` review date"

lapsed_msg := "config/deny.toml: advisory ignore %q lapsed on %s -- drop it, fix the finding, or re-date it"

entries contains entry if {
	some file in input
	file.path == "config/deny.toml"
	some entry in file.contents.advisories.ignore
}

# An entry is either an `{ id, reason }` object or a bare id string; both need naming in a message.
advisory_id(entry) := entry.id if is_object(entry)

advisory_id(entry) := entry if is_string(entry)

dated[id] := date if {
	some entry in entries
	is_object(entry)
	[[_, date]] := regex.find_all_string_submatch_n(expiry_pattern, entry.reason, 1)
	id := entry.id
}

deny contains msg if {
	some entry in entries
	id := advisory_id(entry)
	not dated[id]
	msg := sprintf(undated_msg, [id])
}

# Nanoseconds in a day, added to the parsed midnight so an entry survives the whole of its review date.
day_ns := 86400000000000

deny contains msg if {
	some id, date in dated
	midnight := time.parse_rfc3339_ns(concat("", [date, "T00:00:00Z"]))
	time.now_ns() >= midnight + day_ns
	msg := sprintf(lapsed_msg, [id, date])
}
