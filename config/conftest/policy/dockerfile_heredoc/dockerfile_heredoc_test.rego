# Unit tests for the heredoc first-body-line rule, run by `conftest verify`.
#
# They exist because the rule recognises a heredoc by matching one regex against a raw source line, and a regex
# that stops matching fails open: every Dockerfile then passes the check while nothing is being checked. Running
# the real task over the real Dockerfiles cannot tell that apart from the files being correct, since both look
# like zero denials. Feeding a body that is known to be wrong is what separates the two.
package dockerfile_heredoc_test

import data.dockerfile_heredoc

# One Dockerfile, shaped the way the `ignore` parser hands it over.
# That parser emits every source line as its own {Kind, Original, Value} entry, and conftest wraps a file's
# entries in one more array.
dockerfile(originals) := [{"path": "Dockerfile", "contents": [lines]}] if {
	lines := [line | some o in originals; line := {"Original": o}]
}

# The form CLAUDE.md requires and every Dockerfile in this repo uses: bash first, delimiter quoted.
test_quoted_delimiter_accepts_a_correct_body if {
	msgs := dockerfile_heredoc.deny with input as dockerfile([
		"RUN bash <<'EOF'",
		"set -euo pipefail",
		"apt-get update",
		"EOF",
	])
	count(msgs) == 0
}

test_quoted_delimiter_rejects_a_wrong_first_body_line if {
	msgs := dockerfile_heredoc.deny with input as dockerfile([
		"RUN bash <<'EOF'",
		"apt-get update",
		"EOF",
	])
	count(msgs) == 1
}

# The unquoted form is banned elsewhere, but while it parses as a heredoc this rule still has to see it.
test_unquoted_delimiter_rejects_a_wrong_first_body_line if {
	msgs := dockerfile_heredoc.deny with input as dockerfile(["RUN bash <<EOF", "apt-get update", "EOF"])
	count(msgs) == 1
}

# A RUN carrying mount flags is still a RUN, and the repo has three of them.
test_run_flags_before_the_heredoc_are_tolerated if {
	line := "RUN --mount=type=secret,id=gh_token,required=false bash <<'EOF'"
	msgs := dockerfile_heredoc.deny with input as dockerfile([line, "apt-get update", "EOF"])
	count(msgs) == 1
}

# An unterminated heredoc at EOF leaves no next line to read, which used to panic on the slice index.
test_heredoc_at_end_of_file_does_not_panic if {
	msgs := dockerfile_heredoc.deny with input as dockerfile(["FROM debian", "RUN bash <<'EOF'"])
	count(msgs) == 0
}

# Only a RUN opens a heredoc the rule owns; `<<` elsewhere in the file is someone else's text.
test_non_run_line_is_not_a_heredoc if {
	msgs := dockerfile_heredoc.deny with input as dockerfile(["# see RUN bash <<'EOF' below", "apt-get update"])
	count(msgs) == 0
}
