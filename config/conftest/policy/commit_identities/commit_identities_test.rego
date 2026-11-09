# Unit tests for the coding-agent commit identity policy, run by `conftest verify`.
#
# A branch's real commits rarely credit an agent, so each identity shape the policy recognises is exercised here.
package commit_identities_test

import data.commit_identities

# Identity lines in the shape the ignore parser hands over, with a trailing blank line.
listing(lines) := [{"path": "-", "contents": array.concat(
	[{"Kind": "Path", "Original": line, "Value": line} | some line in lines],
	[{"Kind": "Path", "Original": "", "Value": ""}],
)}]

test_people_github_and_dependency_bots_pass if {
	people := listing([
		"0c84373a author John Vandenberg <jayvdb@gmail.com>",
		"0c84373a committer GitHub <noreply@github.com>",
		"0c84373a trailer Signed-off-by: Pierre Tenedero <pierre.tenedero@gmail.com>",
		"0c84373a trailer Co-authored-by: Claude Martin <claude.martin@example.com>",
		"0c84373a trailer Reviewed-by: Devin Smith <devin@example.com>",
		"0c84373a author dependabot[bot] <49699333+dependabot[bot]@users.noreply.github.com>",
	])
	count(commit_identities.deny) == 0 with input as people
}

test_a_model_named_claude_trailer_is_refused if {
	trailer := listing(["0c84373a trailer Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"])
	msgs := commit_identities.deny with input as trailer
	count(msgs) == 1
	some msg in msgs
	contains(msg, "as Co-Authored-By")
}

test_a_copilot_author_is_refused if {
	author := listing(["1a2b3c4d author Copilot <198982749+Copilot@users.noreply.github.com>"])
	msgs := commit_identities.deny with input as author
	count(msgs) == 1
	some msg in msgs
	contains(msg, "commit 1a2b3c4d")
}

test_a_bot_login_committer_is_refused if {
	address := "136622811+coderabbitai[bot]@users.noreply.github.com"
	committer := listing([$"1a2b3c4d committer coderabbitai[bot] <{address}>"])
	count(commit_identities.deny) == 1 with input as committer
}

test_an_agent_address_under_any_name_is_refused if {
	trailer := listing(["1a2b3c4d trailer Co-authored-by: Agent <cursoragent@cursor.com>"])
	count(commit_identities.deny) == 1 with input as trailer
}

test_an_aider_author_suffix_is_refused if {
	author := listing(["1a2b3c4d author Jane Doe (aider) <jane@example.com>"])
	count(commit_identities.deny) == 1 with input as author
}

test_an_assisted_by_trailer_without_an_address_is_refused if {
	trailer := listing(["1a2b3c4d trailer Assisted-by: Claude:claude-opus-5 coccinelle"])
	count(commit_identities.deny) == 1 with input as trailer
}

test_a_parenthesised_role_note_does_not_hide_the_agent if {
	trailer := listing(["1a2b3c4d trailer Assisted-by: Copilot (code review)"])
	count(commit_identities.deny) == 1 with input as trailer
}
