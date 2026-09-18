# Unit tests for the patch-pin rule, run by `conftest verify`.
#
# They exist because that rule turns entirely on the form of a version requirement, and most of the forms that
# decide it are ones the manifest does not hold: a bare `1.2.3`, a comparator in front of one, a prerelease tail
# whose own dots must not be counted, a path dep's mandatory full triple. Running the real task over the real
# manifest reaches at most one of those at a time, and only for as long as that dep survives a bump, so they are
# fed in synthetically here instead.
package cargo_test

import data.cargo

# A root manifest carrying nothing but the dependency table under test.
manifest(deps) := [{"path": "Cargo.toml", "contents": {"workspace": {"dependencies": deps}}}]

flagged(msgs, name) if {
	some msg in msgs
	contains(msg, sprintf("%q", [name]))
	contains(msg, "to a patch version")
}

stale(msgs, name) if {
	some msg in msgs
	contains(msg, sprintf("patch_pin_exception entry %q is stale", [name]))
}

# The two shapes the rule exists to require, and the one it exists to reject.
test_bare_major_is_accepted if {
	msgs := cargo.deny with input as manifest({"crate-a": "1"})
	not flagged(msgs, "crate-a")
}

test_major_minor_is_accepted if {
	msgs := cargo.deny with input as manifest({"crate-a": "1.2"})
	not flagged(msgs, "crate-a")
}

test_major_minor_patch_is_rejected if {
	msgs := cargo.deny with input as manifest({"crate-a": "1.2.3"})
	flagged(msgs, "crate-a")
}

# The table form carries the requirement under `version`, and must be read the same way as the string form.
test_table_form_is_rejected if {
	msgs := cargo.deny with input as manifest({"crate-a": {"version": "1.2.3", "features": ["derive"]}})
	flagged(msgs, "crate-a")
}

# A comparator must not launder a patch pin past the rule, nor shift the components it counts.
test_comparator_does_not_launder_a_patch_pin if {
	msgs := cargo.deny with input as manifest({"crate-a": "=1.2.3"})
	flagged(msgs, "crate-a")
}

test_comparator_on_major_minor_is_accepted if {
	msgs := cargo.deny with input as manifest({"crate-a": ">=1.2"})
	not flagged(msgs, "crate-a")
}

# A prerelease tail contributes dots of its own, which belong to the tail rather than to the version core.
test_prerelease_counts_only_the_version_core if {
	msgs := cargo.deny with input as manifest({"crate-a": "=2.0.0-rc.10"})
	flagged(msgs, "crate-a")
}

# A path dep has no two-part form available to it, so it is exempt without needing an exception entry.
test_path_dep_keeps_its_full_triple if {
	msgs := cargo.deny with input as manifest({"et-a": {"path": "libs/a", "version": "0.1.0"}})
	not flagged(msgs, "et-a")
}

# An excepted dep keeps its pin, and the entry that permits it is what stops the denial.
test_excepted_dep_keeps_its_patch_pin if {
	msgs := cargo.deny with input as manifest({"deno_error": "=0.7.1", "ort": "=2.0.0-rc.10"})
	not flagged(msgs, "deno_error")
}

test_exception_only_covers_the_dep_it_names if {
	deps := {"deno_error": "=0.7.1", "ort": "=2.0.0-rc.10", "other": "1.2.3"}
	msgs := cargo.deny with input as manifest(deps)
	flagged(msgs, "other")
}

# The map is only honest while every entry still describes the manifest, so both ways of going stale report.
test_trimmed_dep_reports_its_stale_exception if {
	msgs := cargo.deny with input as manifest({"deno_error": "=0.7.1", "ort": "2.0"})
	stale(msgs, "ort")
}

test_dropped_dep_reports_its_stale_exception if {
	msgs := cargo.deny with input as manifest({"deno_error": "=0.7.1"})
	stale(msgs, "ort")
}

test_live_exceptions_report_nothing if {
	msgs := cargo.deny with input as manifest({"deno_error": "=0.7.1", "ort": "=2.0.0-rc.10"})
	not stale(msgs, "deno_error")
	not stale(msgs, "ort")
}
