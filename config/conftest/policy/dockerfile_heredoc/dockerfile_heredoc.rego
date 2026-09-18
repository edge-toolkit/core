# Body-first-line rule for Dockerfile heredocs, requiring `set -euo pipefail` as the first body line of a `RUN <<...`.
# This runs separately from the `dockerfile` package because conftest's dockerfile parser flattens heredoc bodies out
# of the AST. The `ignore` parser instead emits every line of the file as a separate entry ({Kind, Original, Value}),
# so we can look at the line index after a `RUN <<...` and check the next line is `set -euo pipefail`. Paired with the
# `dockerfile` package's rule that the heredoc interpreter must be `bash`.
package dockerfile_heredoc

# Each parsed file is an array of entries; conftest wraps them per file as input[N].contents = [[entry, entry, ...]].
# The outer wrapping array is the whole file's set of items.
entries(file) := items if {
	is_array(file.contents)
	items := file.contents[0]
}

deny contains msg if {
	some file in input
	items := entries(file)
	some i, entry in items
	is_string(entry.Original)

	# Everything between `<<` and the delimiter tag's first character is optional.
	# The tag may be quoted (`<<'EOF'`, the form this repo requires) or bare, and `<<-` strips leading tabs, so
	# a pattern that insists on the tag starting immediately after `<<` sees only the bare form.
	regex.match(`^RUN[^\n]*<<-?['"]?[A-Za-z_]`, entry.Original)

	# Bounds-check before indexing to avoid a `panic: slice bounds out of range`.
	# An unterminated heredoc at EOF would otherwise trip it.
	i + 1 < count(items)
	next := items[i + 1]
	not next.Original == "set -euo pipefail"
	msg := sprintf(
		"%s: RUN heredoc at line ~%d must have `set -euo pipefail` as its first body line, got %q",
		[file.path, i + 1, next.Original],
	)
}
