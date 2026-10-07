# The README's checks badge, held to the rules count CHECKS.md records.
# Run with `--namespace checks_badge` over CHECKS.md and README.md, `--combine`d and parsed with `ignore`, which
# hands each file over as its lines; the `Original` field of each is the line's text.
package checks_badge

# How far the rules count moves before the badge has to follow: the badge records it rounded down to this.
step := 20

badge_prefix := "[checks-badge]: "

# The rules count from the `- **Rules:** N ...` line of CHECKS.md's header.
rules_count := to_number(match[1]) if {
	some file in input
	endswith(file.path, "CHECKS.md")
	some line in file.contents
	some match in regex.find_all_string_submatch_n(`^- \*\*Rules:\*\* ([0-9]+) `, line.Original, 1)
}

# The badge line for the rules count rounded down to a multiple of `step`.
expected_badge := concat("", [badge_prefix, badge_url_prefix, format_int(rounded, 10), "-blue"]) if {
	rounded := rules_count - (rules_count % step)
}

badge_url_prefix := "https://img.shields.io/badge/checks-%3E%3D"

badge_lines contains line.Original if {
	some file in input
	endswith(file.path, "README.md")
	some line in file.contents
	startswith(line.Original, badge_prefix)
}

# CHECKS.md must record its rules count, which the badge is held to.
deny contains msg if {
	not rules_count
	msg := "CHECKS.md: no `- **Rules:** N` line to hold the README's checks badge to"
}

# README.md must define the checks badge.
deny contains msg if {
	count(badge_lines) == 0
	msg := $"README.md: no `{badge_prefix}` line; add one recording the rules count CHECKS.md records"
}

# The checks badge records the rules count rounded down to a multiple of 20, so it moves with every 20 rules added.
deny contains msg if {
	some line in badge_lines
	line != expected_badge
	msg := $"README.md: checks badge is `{line}`, but CHECKS.md's rules count needs `{expected_badge}`"
}
