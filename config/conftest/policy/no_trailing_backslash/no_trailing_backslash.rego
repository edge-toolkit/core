# Defense-in-depth duplicate of the repo-wide raw-text trailing-backslash ban.
# Flags the trailing-backslash line continuation we don't want anywhere in the repo, by walking every string
# anywhere in the combined input and flagging any literal `\` immediately followed by a newline.
#
# Scope:
# - TOML (`--namespace no_trailing_backslash` on conftest-check-toml): catches continuations embedded in multi-line
#   `"""..."""` task `run` bodies, which the TOML parser preserves verbatim in the parsed string.
# - YAML (`--namespace no_trailing_backslash` on conftest-check-yaml): catches continuations inside `|`/`>`
#   block-scalar values, similarly preserved by the YAML parser.
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
