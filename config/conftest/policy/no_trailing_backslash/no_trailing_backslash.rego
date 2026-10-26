# Defense-in-depth duplicate of the repo-wide raw-text trailing-backslash ban.
# Flags the trailing-backslash line continuation we don't want anywhere in the repo, by walking every string
# anywhere in the combined input and flagging any literal `\` immediately followed by a newline.
#
# Scope:
# - TOML (every tracked file, in conftest-check-toml): catches continuations inside multi-line `'''...'''` literal
#   strings, which the TOML parser preserves verbatim. A `"""..."""` basic string is not covered: TOML itself folds a
#   line-ending `\` and its newline away before Rego sees the value, so only the raw-text rule can reach those.
# - YAML (every tracked file, in conftest-check-yaml): catches continuations inside `|`/`>` block-scalar values,
#   similarly preserved by the YAML parser.
# The rule walks `input` as a list of files, so it only ever sees anything when run with `--combine`.
# - Dockerfile: NOT covered here -- conftest's dockerfile parser already consumes line-continuation backslashes when
#   it joins each instruction's value, so by the time Rego sees the parsed input the backslashes are gone. The
#   raw-text rule is the source of truth for Dockerfile coverage, and allowlists `Dockerfile*` there because
#   Dockerfile RUN bodies legitimately need line continuations.
package no_trailing_backslash

# Flag any string leaf containing `\` followed by `\n` (LF), reporting its path key in the message.
# The path key is an array of indices/keys from the root of the parsed document down to the offending string.
# Rego's `walk` yields [path, value] pairs over every node, so we filter to string leaves and regex-test for `\`
# followed by `\n` (LF).
deny contains msg if {
	some file in input
	walk(file.contents, [path, value])
	is_string(value)
	regex.match(`\\\n`, value)
	msg := sprintf(
		"%s: trailing-backslash line continuation in string at %v -- the form is banned repo-wide",
		[file.path, path],
	)
}
