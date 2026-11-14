# Unit tests for the cargo/mise publish-age cross-check, run by `conftest verify`.
#
# The real task only ever sees two ages that already agree, which looks the same as a rule that never matches.
package cross_test

import data.cross

age_cargo_file(cargo) := {"path": ".cargo/config.toml", "contents": cargo}

age_mise_file(mise) := {"path": ".mise/config.toml", "contents": mise}

age_input(cargo_age, mise_age) := [
	age_cargo_file({"registry": {"global-min-publish-age": cargo_age}}),
	age_mise_file({"settings": {"minimum_release_age": mise_age}}),
]

age_reports(msgs, fragment) if {
	some msg in msgs
	startswith(msg, "publish age:")
	contains(msg, fragment)
}

age_clean(msgs) if {
	not any_age_report(msgs)
}

any_age_report(msgs) if {
	some msg in msgs
	startswith(msg, "publish age:")
}

test_age_days_matching_is_accepted if {
	age_clean(cross.deny) with input as age_input("7 days", "7d")
}

test_age_singular_week_matching_is_accepted if {
	age_clean(cross.deny) with input as age_input("1 week", "1w")
}

test_age_windows_paths_are_still_read if {
	files := age_input("7 days", "7d")
	cargo := object.union(files[0], {"path": `.cargo\config.toml`})
	windows := [cargo, object.union(files[1], {"path": `.mise\config.toml`})]
	age_clean(cross.deny) with input as windows
}

test_age_missing_cargo_config_is_flagged if {
	msgs := cross.deny with input as [age_mise_file({"settings": {"minimum_release_age": "7d"}})]
	age_reports(msgs, ".cargo/config.toml is missing")
}

test_age_missing_cargo_key_is_flagged if {
	files := [age_cargo_file({"registry": {}}), age_mise_file({"settings": {"minimum_release_age": "7d"}})]
	msgs := cross.deny with input as files
	age_reports(msgs, "sets no [registry] global-min-publish-age")
}

test_age_missing_mise_key_is_flagged if {
	files := [age_cargo_file({"registry": {"global-min-publish-age": "7 days"}}), age_mise_file({"settings": {}})]
	msgs := cross.deny with input as files
	age_reports(msgs, "sets no [settings] minimum_release_age")
}

test_age_bare_zero_is_flagged if {
	msgs := cross.deny with input as age_input("0", "0")
	age_reports(msgs, "global-min-publish-age \"0\" is not")
	age_reports(msgs, "minimum_release_age \"0\" is not")
}

test_age_zero_with_unit_is_flagged if {
	msgs := cross.deny with input as age_input("0 days", "0d")
	age_reports(msgs, "must not be zero")
}

test_age_months_is_flagged if {
	msgs := cross.deny with input as age_input("7 months", "7m")
	age_reports(msgs, "global-min-publish-age \"7 months\" is not")
}

test_age_unparsable_mise_value_is_flagged if {
	msgs := cross.deny with input as age_input("7 days", "2026-01-01")
	age_reports(msgs, "minimum_release_age \"2026-01-01\" is not")
}

test_age_different_numbers_are_flagged if {
	msgs := cross.deny with input as age_input("5 days", "7d")
	age_reports(msgs, "\"5 days\" != mise minimum_release_age \"7d\"")
}

test_age_different_units_are_flagged if {
	msgs := cross.deny with input as age_input("7 weeks", "7d")
	age_reports(msgs, "\"7 weeks\" != mise minimum_release_age \"7d\"")
}

age_with_registry(registry, registries) := [
	age_cargo_file({"registry": registry, "registries": registries}),
	age_mise_file({"settings": {"minimum_release_age": "7d"}}),
]

test_age_matching_registry_override_is_accepted if {
	files := age_with_registry({"global-min-publish-age": "7 days"}, {"crates-io": {"min-publish-age": "7 days"}})
	age_clean(cross.deny) with input as files
}

test_age_zero_registry_override_is_flagged if {
	files := age_with_registry({"global-min-publish-age": "7 days"}, {"crates-io": {"min-publish-age": "0"}})
	msgs := cross.deny with input as files
	age_reports(msgs, "[registries.crates-io] min-publish-age \"0\" != mise")
}

test_age_shorter_crates_io_age_is_flagged if {
	registry := {"global-min-publish-age": "7 days", "min-publish-age": "5 days"}
	msgs := cross.deny with input as age_with_registry(registry, {})
	age_reports(msgs, "[registry] min-publish-age \"5 days\" != mise")
}

test_age_committed_allow_is_flagged if {
	cargo := {"registry": {"global-min-publish-age": "7 days"}, "resolver": {"incompatible-publish-age": "allow"}}
	files := [age_cargo_file(cargo), age_mise_file({"settings": {"minimum_release_age": "7d"}})]
	msgs := cross.deny with input as files
	age_reports(msgs, "incompatible-publish-age")
}
