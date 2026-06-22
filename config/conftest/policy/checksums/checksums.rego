# Bidirectional cross-reference between .mise/config*.toml's `<name>_asset` vars and
# config/upstream-cache/data.toml's `[asset.<filename>]` tables. Run with `--namespace checksums`; the
# conftest-check-toml task includes both file sets in its `--combine` input.
#
# Forward: a `*_asset` var must have a matching entry recorded, or its fetch-* task would download something we can't
# integrity-verify. Reverse: a `[asset.<filename>]` table must be referenced by a `*_asset` var, or it is a stale
# record from a refresh someone forgot to wire all the way through.
#
# Each `[asset.<filename>]` table must carry `url` (the canonical download URL), `upstream` (project/dataset the asset
# was built from), and `license` (SPDX expression). `sha256` may be empty (`""`) while we bootstrap a new
# upstream-cache release before the first upload -- every other field is required.
package checksums

is_mise(file) if startswith(file.path, ".mise/config")

is_upstream_cache(file) if file.path == "config/upstream-cache/data.toml"

# Module-scope sprintf templates kept above the 120-char editorconfig limit only by living on their own lines.
missing_sha256_msg := "config/upstream-cache/data.toml: [asset.%q] is missing `sha256` (use `\"\"` while bootstrapping)"

# Asset filenames declared in any .mise/config*.toml's [vars].
mise_assets contains filename if {
	some file in input
	is_mise(file)
	some key, filename in file.contents.vars
	endswith(key, "_asset")
}

# Filenames present in config/upstream-cache/data.toml's [asset.*] tables.
recorded_assets contains filename if {
	some file in input
	is_upstream_cache(file)
	some filename, _ in file.contents.asset
}

deny contains msg if {
	some asset in mise_assets
	not asset in recorded_assets
	msg := sprintf(
		"config/upstream-cache/data.toml: missing [asset.%q] table (referenced via a `_asset` var in .mise/config*.toml)",
		[asset],
	)
}

deny contains msg if {
	some asset in recorded_assets
	not asset in mise_assets
	msg := sprintf(
		"config/upstream-cache/data.toml: [asset.%q] is recorded but no `_asset` var in .mise/config*.toml references it",
		[asset],
	)
}

# Per-entry shape check: url + license are required strings; sha256 must
# be present (may be empty during bootstrap).
deny contains msg if {
	some file in input
	is_upstream_cache(file)
	some filename, entry in file.contents.asset
	not entry.url
	msg := sprintf("config/upstream-cache/data.toml: [asset.%q] is missing `url` (the download URL)", [filename])
}

deny contains msg if {
	some file in input
	is_upstream_cache(file)
	some filename, entry in file.contents.asset
	not entry.upstream
	msg := sprintf("config/upstream-cache/data.toml: [asset.%q] is missing `upstream` (the project URL)", [filename])
}

deny contains msg if {
	some file in input
	is_upstream_cache(file)
	some filename, entry in file.contents.asset
	not entry.license
	msg := sprintf("config/upstream-cache/data.toml: [asset.%q] is missing `license` (SPDX expression)", [filename])
}

deny contains msg if {
	some file in input
	is_upstream_cache(file)
	some filename, entry in file.contents.asset
	not entry.sha256
	msg := sprintf(missing_sha256_msg, [filename])
}
