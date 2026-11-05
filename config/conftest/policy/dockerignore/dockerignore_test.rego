# Unit tests for the top-level Markdown dockerignore policy, run by `conftest verify`.
#
# The real task only sees a .dockerignore already in step, which looks the same as a rule that never matches.
package dockerignore_test

import data.dockerignore

combined(texts) := [{
	"path": ".dockerignore",
	"contents": [{"Kind": "Path", "Original": text, "Value": text} | some text in texts],
}]

reports(msgs, fragment) if {
	some msg in msgs
	contains(msg, fragment)
}

test_the_top_level_markdown_exclude_is_accepted if {
	count(dockerignore.deny) == 0 with input as combined(["**/target/", ".git/", "Dockerfile*", "*.md"])
}

test_excluding_only_the_readme_is_flagged if {
	msgs := dockerignore.deny with input as combined(["**/target/", ".git/", "Dockerfile*", "README.md"])
	reports(msgs, "no `*.md` line")
}

test_a_negation_before_the_exclude_is_overridden_and_accepted if {
	count(dockerignore.deny) == 0 with input as combined(["!README.md", "*.md"])
}

test_bringing_a_top_level_file_back_is_flagged if {
	msgs := dockerignore.deny with input as combined(["*.md", "!CHANGELOG.md"])
	reports(msgs, "`!CHANGELOG.md` follows the last `*.md`")
}

test_bringing_a_root_anchored_file_back_is_flagged if {
	msgs := dockerignore.deny with input as combined(["*.md", "!/README.md"])
	reports(msgs, "`!/README.md` follows the last `*.md`")
}

test_bringing_a_file_back_by_wildcard_is_flagged if {
	msgs := dockerignore.deny with input as combined(["*.md", "!README.*"])
	reports(msgs, "`!README.*` follows the last `*.md`")
}

test_bringing_a_file_back_at_any_depth_is_flagged if {
	msgs := dockerignore.deny with input as combined(["*.md", "!**/README.md"])
	reports(msgs, "`!**/README.md` follows the last `*.md`")
}

test_only_the_last_exclude_counts if {
	msgs := dockerignore.deny with input as combined(["*.md", "!README.md", "*.md"])
	count(msgs) == 0
}

test_bringing_a_nested_readme_back_is_accepted if {
	count(dockerignore.deny) == 0 with input as combined(["*.md", "!services/ws-server/README.md"])
}

test_bringing_back_a_file_that_cannot_be_markdown_is_accepted if {
	count(dockerignore.deny) == 0 with input as combined(["*.md", "!**/*.wasm"])
}
