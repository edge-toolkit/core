# Unit tests for the upstream-cache cross-reference rule, run by `conftest verify`.
#
# They exist because both directions of the cross-reference fail open when their key matching breaks: a var
# whose `_asset` suffix stops being recognised, or an asset table that stops being read, produces an empty set
# on one side and an empty set cross-references cleanly against anything. The real task only ever sees a
# manifest that already agrees, so it cannot tell a working cross-check from one that compares nothing.
package checksums_test

import data.checksums

entry := {
	"sha256": "3b2c1d",
	"url": "https://example.invalid/releases/download/v1/tool.tar.gz",
	"upstream": "https://example.invalid/tool",
	"license": "Apache-2.0",
}

scenario(vars, assets) := [
	{"path": ".mise/config.toml", "contents": {"vars": vars}},
	{"path": "config/upstream-cache/data.toml", "contents": {"asset": assets}},
]

says(msgs, fragment) if {
	some msg in msgs
	contains(msg, fragment)
}

test_matched_var_and_asset_table_are_clean if {
	msgs := checksums.deny with input as scenario({"tool_asset": "tool.tar.gz"}, {"tool.tar.gz": entry})
	count(msgs) == 0
}

test_var_without_an_asset_table_is_flagged if {
	msgs := checksums.deny with input as scenario({"tool_asset": "tool.tar.gz"}, {})
	says(msgs, "missing [asset.")
}

test_asset_table_without_a_var_is_flagged if {
	msgs := checksums.deny with input as scenario({}, {"tool.tar.gz": entry})
	says(msgs, "is recorded but no")
}

# Only a `_asset` var names an asset; every other var in the same table is someone else's.
test_other_vars_are_not_read_as_asset_names if {
	msgs := checksums.deny with input as scenario({"tool_url": "tool.tar.gz"}, {})
	count(msgs) == 0
}

# An empty sha256 is the documented bootstrap state, held open until the first upload lands.
test_empty_sha256_is_allowed_while_bootstrapping if {
	bootstrapping := object.union(entry, {"sha256": ""})
	msgs := checksums.deny with input as scenario({"tool_asset": "tool.tar.gz"}, {"tool.tar.gz": bootstrapping})
	count(msgs) == 0
}

# An absent sha256 is not the same as an empty one -- it means nobody has decided yet.
test_absent_sha256_is_flagged if {
	stripped := object.remove(entry, ["sha256"])
	msgs := checksums.deny with input as scenario({"tool_asset": "tool.tar.gz"}, {"tool.tar.gz": stripped})
	says(msgs, "is missing `sha256`")
}

test_absent_url_is_flagged if {
	stripped := object.remove(entry, ["url"])
	msgs := checksums.deny with input as scenario({"tool_asset": "tool.tar.gz"}, {"tool.tar.gz": stripped})
	says(msgs, "is missing `url`")
}

test_absent_upstream_is_flagged if {
	stripped := object.remove(entry, ["upstream"])
	msgs := checksums.deny with input as scenario({"tool_asset": "tool.tar.gz"}, {"tool.tar.gz": stripped})
	says(msgs, "is missing `upstream`")
}

test_absent_license_is_flagged if {
	stripped := object.remove(entry, ["license"])
	msgs := checksums.deny with input as scenario({"tool_asset": "tool.tar.gz"}, {"tool.tar.gz": stripped})
	says(msgs, "is missing `license`")
}
