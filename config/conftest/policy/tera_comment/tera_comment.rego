# Bans the Tera comment opener `{#` from every value of a mise config.
# mise renders each task body and value as a Tera template before any shell sees it, and Tera reads `{#` as the
# start of a comment running to the next `#}`. A bash `${#var}` or `${#array[@]}` therefore never reaches bash: the
# task fails to render, before its first command runs and on every platform alike, with
#
#   Closing comment tag `#}` not found
#
# as the k3s cluster check did on commit
# https://github.com/edge-toolkit/core/commit/32aafa2a5f034918d1d2643e7aaf3890b01edc86 at
# https://github.com/edge-toolkit/core/actions/runs/38812052621/job/116687432014. Checked over the parsed TOML
# rather than its text, because parsing drops TOML comments -- which mise never renders, so a comment documenting
# the pitfall is free to spell it out -- and keeps every string mise does render, task bodies included.
package tera_comment

# A mise config value must not contain `{#`, which Tera reads as the start of a comment.
deny contains msg if {
	some file in input
	walk(file.contents, [path, value])
	is_string(value)
	contains(value, "{#")
	msg := sprintf(
		concat("", [
			"%s: value at %v contains `{#`, which mise's Tera rendering reads as a comment opener -- count without a ",
			"`${#...}` length: keep a counter, test the value itself for emptiness, or ask the input for the count",
		]),
		[file.path, path],
	)
}
