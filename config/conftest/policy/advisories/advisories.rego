# Review dates on the advisory ignores in config/deny.toml and on the scanner's two ignore files.
# Run with `--namespace advisories` by conftest-check-toml, which feeds all three in one --combine run, so `input`
# is the array of {path, contents} rather than one file's contents.
#
# Every entry in `[advisories].ignore` is an accepted finding, and an accepted finding that nobody looks at
# again is indistinguishable from one nobody noticed. So each carries a review date in its `reason`, and this
# fails the build once that date passes -- the entry then has to be dropped, fixed, or consciously re-dated,
# which is a reviewable one-line diff rather than silence.
#
# The undated rule is what keeps that true of future entries: a new ignore added as a bare id, or with a
# reason this cannot parse, fails immediately rather than quietly becoming the one exception with no deadline.
package advisories

import data.paths

# The `reason` spelling the date is read from, anchored so trailing prose cannot hide a second date.
expiry_pattern := `^expires ([0-9]{4}-[0-9]{2}-[0-9]{2})$`

undated_msg := "config/deny.toml: advisory ignore %q has no `reason = \"expires YYYY-MM-DD\"` review date"

lapsed_msg := "config/deny.toml: advisory ignore %q lapsed on %s -- drop it, fix the finding, or re-date it"

mixed_msg := "config/deny.toml: advisory ignores expire on %d dates, %v; a new entry takes the date the others carry"

entries contains entry if {
	some file in paths.files
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

# Every advisory ignore must carry a `reason = "expires YYYY-MM-DD"` review date.
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
	midnight := time.parse_rfc3339_ns($"{date}T00:00:00Z")
	time.now_ns() >= midnight + day_ns
	msg := sprintf(lapsed_msg, [id, date])
}

# Every distinct review date the advisory ignores carry; one, once the rule below holds.
review_dates := {date | some date in dated}

# Every advisory ignore expires on one date, the next milestone review, which the operator sets.
deny contains msg if {
	count(review_dates) > 1
	msg := sprintf(mixed_msg, [count(review_dates), sort(review_dates)])
}

# The scanner's two ignore files are reviewed on the same milestone as deny.toml.
# One holds the accepted vulnerabilities inside the mise-installed tools, the other the accepted Dockerfile and
# manifest misconfigurations. Each entry's `expired_at` is the scanner's own lapse date, so its scan already fails
# once that passes; what these rules add is that the date cannot be missing or drift from the one the operator sets
# in deny.toml.
tools_file := "config/trivyignore-tools.yaml"

# Each dated ignore file, and the list in it that holds the entries.
expiring_lists := {tools_file: "vulnerabilities", "config/trivyignore.yaml": "misconfigurations"}

undated_finding_msg := "%s: accepted finding %q has no `expired_at` review date"

drift_msg := "%s: accepted finding %q expires on %s, not on %v like config/deny.toml"

expiring contains [file.path, entry] if {
	some file in paths.files
	some entry in file.contents[expiring_lists[file.path]]
}

tool_entries contains entry if {
	some [path, entry] in expiring
	path == tools_file
}

has_expiry(entry) if is_string(entry.expired_at)

# Every entry in the scanner's ignore files must carry an `expired_at` review date.
deny contains msg if {
	some [path, entry] in expiring
	not has_expiry(entry)
	msg := sprintf(undated_finding_msg, [path, entry.id])
}

# Every entry in the scanner's ignore files expires on the date the advisory ignores in config/deny.toml carry.
deny contains msg if {
	some [path, entry] in expiring
	has_expiry(entry)
	not entry.expired_at in review_dates
	msg := sprintf(drift_msg, [path, entry.id, entry.expired_at, sort(review_dates)])
}

# The accepted advisories published before 2026, each one reviewed and named here.
# That is one from 2023, four from 2024 and eight from 2025. An advisory that old has had a fix available for a long
# time, so accepting another one is a decision to make here, in review, rather than an entry that slips into an ignore
# list unnoticed. The list only shrinks: an id that leaves every ignore list has to leave this one too.
old_advisories := {
	"RUSTSEC-2023-0071",
	"RUSTSEC-2024-0436",
	"RUSTSEC-2024-0449",
	"CVE-2024-21534",
	"CVE-2024-34156",
	"RUSTSEC-2025-0134",
	"CVE-2025-1302",
	"CVE-2025-15558",
	"CVE-2025-47273",
	"CVE-2025-47913",
	"CVE-2025-61726",
	"CVE-2025-61729",
	"CVE-2025-68121",
}

# The first year an advisory may come from without being named in `old_advisories`.
current_year := 2026

# The year in an advisory id: CVE, RUSTSEC, PYSEC, GO and the other `<PREFIX>-<year>-<number>` schemes.
# Anchored at both ends so a GHSA id, whose three segments are base-32 rather than a year and a number, never reads
# as dated, even when its first segment happens to be four digits.
year_pattern := `^[A-Z]+-([0-9]{4})-[0-9]+$`

old_msg := "%s: accepted advisory %q is from %d; one from before %d must be named in advisories.rego's old_advisories"

stale_msg := "advisories.rego: old_advisories names %q, which no ignore list accepts any more -- remove it"

# Every accepted advisory id, with the file that accepts it.
accepted contains ["config/deny.toml", advisory_id(entry)] if {
	some entry in entries
}

accepted contains [tools_file, entry.id] if {
	some entry in tool_entries
}

advisory_year(id) := to_number(year) if {
	[[_, year]] := regex.find_all_string_submatch_n(year_pattern, id, 1)
}

# An advisory published before 2026 may only be accepted if old_advisories names it.
deny contains msg if {
	some [path, id] in accepted
	year := advisory_year(id)
	year < current_year
	not id in old_advisories
	msg := sprintf(old_msg, [path, id, year, current_year])
}

accepted_ids contains id if {
	some [_, id] in accepted
}

# Staleness can only be judged from every ignore list at once; a run fed fewer files cannot tell gone from unread.
every_list_read if {
	read := {file.path | some file in paths.files}
	"config/deny.toml" in read
	tools_file in read
}

# Every id in old_advisories must still be accepted by some ignore list, so the allowlist cannot outlive its entries.
deny contains msg if {
	every_list_read
	some id in old_advisories
	not id in accepted_ids
	msg := sprintf(stale_msg, [id])
}
