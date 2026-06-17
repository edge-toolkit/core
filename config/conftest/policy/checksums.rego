# Bidirectional cross-reference between .mise/config*.toml's `<name>_asset`
# vars and config/checksums.toml's [sha256] table. Run with
# `--namespace checksums`; the conftest-check-toml task includes both file
# sets in its `--combine` input.
#
# Forward: a `*_asset` var must have a matching checksum recorded, or its
# fetch-* task would download something we can't integrity-verify.
# Reverse: a checksum entry must be referenced by a `*_asset` var, or it
# is a stale record from a refresh someone forgot to wire all the way
# through (filename in checksums.toml but no var pointing at it).
package checksums

is_mise(file) if startswith(file.path, ".mise/config")

is_checksums(file) if file.path == "config/checksums.toml"

# Asset filenames declared in any .mise/config*.toml's [vars].
mise_assets contains filename if {
	some file in input
	is_mise(file)
	some key, val in file.contents.vars
	endswith(key, "_asset")
	filename := val
}

# Filenames present in config/checksums.toml's [sha256] table.
recorded_assets contains filename if {
	some file in input
	is_checksums(file)
	some filename, _ in file.contents.sha256
}

deny contains msg if {
	some asset in mise_assets
	not asset in recorded_assets
	msg := sprintf(
		"config/checksums.toml: missing [sha256] entry for %q (referenced via a `_asset` var in .mise/config*.toml)",
		[asset],
	)
}

deny contains msg if {
	some asset in recorded_assets
	not asset in mise_assets
	msg := sprintf(
		"config/checksums.toml: [sha256].%q is recorded but no `_asset` var in .mise/config*.toml references it",
		[asset],
	)
}
