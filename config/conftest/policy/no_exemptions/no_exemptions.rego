# Rules held to every file in the repository, whose configs may therefore never narrow what they scan.
# Run with `--combine --namespace no_exemptions` over config/ast-grep/rules and config/semgrep, so every rule file
# arrives as one `{path, contents}` entry and a rule here can see them all at once.
#
# A rule listed here applies everywhere or it guarantees nothing: a path carve-out added to make one file pass also
# removes the guarantee from every file the glob matches later, and a generated tree carved out because "the
# generator owns it" let em-dashes accumulate unseen. A file that breaks such a rule is fixed, and output a generator
# writes is fixed in the generator. The other way to exempt a file is inline, with a suppression comment; the
# semgrep rule named by `inline_guard` flags those, and the last two rules below keep it in step with this list.

# METADATA
# custom:
#   meta: true
package no_exemptions

# The rules no config may narrow and no comment may suppress, by id, whichever linter carries them.
unexemptable := {"no-non-ascii"}

# The semgrep rule flagging inline suppressions of the rules above.
inline_guard := "no-suppressing-unexemptable-rules"

# Module-scope sprintf templates, each on its own line so the source stays inside the line limit.
scoping_msg := "%s: %q applies to every file, so it may not set %q to narrow which files it scans"

unnamed_msg := "%s: %q must flag inline suppressions of %q, but none of its patterns names that rule"

missing_msg := "no semgrep rule %q flags inline suppressions of the unexemptable rules %v"

# Every semgrep rule in the input, paired with the file defining it.
semgrep_rules contains [file.path, rule] if {
	some file in input
	some rule in file.contents.rules
}

# An unexemptable semgrep rule may not set `paths`, which is how semgrep narrows the files a rule scans.
deny contains msg if {
	some [path, rule] in semgrep_rules
	rule.id in unexemptable
	rule.paths
	msg := sprintf(scoping_msg, [path, rule.id, "paths"])
}

# An unexemptable ast-grep rule may not set `files` or `ignores`, the two ways ast-grep narrows a rule.
deny contains msg if {
	some file in input
	file.contents.id in unexemptable
	some key in {"files", "ignores"}
	file.contents[key]
	msg := sprintf(scoping_msg, [file.path, file.contents.id, key])
}

# Every detection pattern of a semgrep rule, however its `pattern-either` / `patterns` lists nest them.
# Only these decide what the rule matches, so an id named in its message or a comment does not count.
detection_patterns(rule) := {value |
	walk(rule, [path, value])
	path[count(path) - 1] in {"pattern", "pattern-regex"}
	is_string(value)
}

# Whether one of `rule`'s detection patterns names `id`.
pattern_names(rule, id) if {
	some pattern in detection_patterns(rule)
	contains(pattern, id)
}

# The inline guard must name every unexemptable rule, or suppressing one added later would go unflagged.
deny contains msg if {
	some [path, rule] in semgrep_rules
	rule.id == inline_guard
	some id in unexemptable
	not pattern_names(rule, id)
	msg := sprintf(unnamed_msg, [path, inline_guard, id])
}

# The inline guard must exist, since without it a suppression comment is an exemption nothing reports.
deny contains msg if {
	count({rule | some [_, rule] in semgrep_rules; rule.id == inline_guard}) == 0
	msg := sprintf(missing_msg, [inline_guard, unexemptable])
}
