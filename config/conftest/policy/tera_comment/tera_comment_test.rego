# Unit tests for the Tera comment-opener rule, run by `conftest verify`.
#
# The rule fails open: a `walk` that stops descending, or a match that stops matching, leaves every config
# passing, which reads exactly like a clean repo when the only evidence is a run over files that already comply.
package tera_comment_test

import data.tera_comment

parsed(contents) := [{"path": ".mise/config.toml", "contents": contents}]

test_a_length_expansion_in_a_task_body_is_caught if {
	body := "if [ \"${#items[@]}\" -eq 0 ]; then\n  exit 1\nfi\n"
	count(tera_comment.deny) == 1 with input as parsed({"tasks": {"probe": {"run": body}}})
}

test_a_value_nested_in_an_env_table_is_caught if {
	count(tera_comment.deny) == 1 with input as parsed({"env": {"COUNT": "${#LIST}"}})
}

# Tera's own tags are what the configs use deliberately, so only the comment opener is banned.
test_tera_tags_and_plain_braces_pass if {
	values := {"vars": {
		"arch": "{% if arch() == 'arm64' %}aarch64{% else %}x86_64{% endif %}",
		"shell": "{{ vars.task_shell }}",
		"jsonpath": "{.items[0].metadata.name}",
	}}
	count(tera_comment.deny) == 0 with input as parsed(values)
}
