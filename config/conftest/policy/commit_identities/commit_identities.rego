# Coding agents credited in the commits a branch adds, as author, committer or a `-by` trailer.
# Run with `--combine --parser ignore --namespace commit_identities` over the identity lines on stdin, so `input` is a
# single entry whose `contents` are those lines. Each line is `<commit> author <value>`, `<commit> committer <value>`
# or `<commit> trailer <Key>-by: <value>`, the last for every message line whose key ends in `-by` -- Co-authored-by,
# Signed-off-by, Co-developed-by, Assisted-by, Reviewed-by and the rest -- wherever it sits in the message.
#
# Every contributor signs the Contributor License Agreement in CLA.md, and a coding agent cannot sign anything, so a
# commit crediting one carries work that nobody has licensed to the project. CLA assistant checks commit authors but
# not trailers, which would let a `Co-authored-by` line naming an agent through unseen.
#
# Agents are recognised by the identities they commit under, never by a vendor's whole email domain, so people who
# work at those vendors can still contribute under their own addresses. Extend the sets as new agents appear.
package commit_identities

# Addresses agents commit or sign under.
agent_emails := {
	"agent@cursor.sh",
	"aider@aider.chat",
	"copilot@github.com",
	"cursoragent@cursor.com",
	"noreply@aider.chat",
	"noreply@anthropic.com",
	"openhands@all-hands.dev",
}

# GitHub logins of agent accounts and apps, as a `users.noreply.github.com` address carries them.
agent_logins := {
	"amazon-q-developer",
	"chatgpt-codex-connector",
	"claude",
	"coderabbitai",
	"copilot",
	"copilot-swe-agent",
	"cursor",
	"devin-ai-integration",
	"gemini-code-assist",
	"google-labs-jules",
	"openhands-agent",
	"sweep-ai",
}

# Names agents commit under, matched whole so that a person who merely shares a first name is not caught.
agent_names := {
	"aider",
	"amazon q",
	"amazon q developer",
	"claude",
	"claude code",
	"cline",
	"codex",
	"coderabbit",
	"copilot",
	"cursor",
	"cursor agent",
	"devin",
	"gemini",
	"gemini cli",
	"gemini code assist",
	"github copilot",
	"jules",
	"openai codex",
	"openhands",
	"windsurf",
}

# Name shapes carrying a model or tool: `Claude Opus 5.5`, `Claude:claude-opus-5`, `aider (gpt-5)`, `Jane (aider)`.
agent_name_patterns := [
	`^claude( code\b| (opus|sonnet|haiku|fable|instant)\b|[-:])`,
	`^aider \(`,
	`\(aider\)$`,
]

agent_msg := "commit %s credits the coding agent %q as %s, and an agent cannot sign CLA.md -- remove or re-author it"

# Every identity line, blank lines aside.
lines contains line.Original if {
	some file in input
	some line in file.contents
	line.Original != ""
}

# Each identity line split into its commit, where the identity appears, and the credited value.
identities contains {"commit": parts[1], "where": where(parts[2], parts[3]), "value": credited(parts[2], parts[3])} if {
	some line in lines
	some parts in regex.find_all_string_submatch_n(`^(\S+) (author|committer|trailer) (.+)$`, line, 1)
}

# Where an identity appears: the author or committer field, or the key of the trailer naming it.
where("trailer", text) := trim_space(substring(text, 0, indexof(text, ":")))

where(role, _) := role if role != "trailer"

# The identity a line credits: a trailer's value after its key, or the whole author or committer field.
credited("trailer", text) := trim_space(substring(text, indexof(text, ":") + 1, -1))

credited(role, text) := text if role != "trailer"

# The lowercased address in a trailing `<email>`, undefined when the value carries none.
email_of(value) := lower(parts[1]) if {
	some parts in regex.find_all_string_submatch_n(`<([^>]*)>\s*$`, value, 1)
}

# The lowercased name before any `<email>`.
display_name(value) := lower(trim_space(regex.replace(value, `<[^>]*>\s*$`, "")))

# The name with a trailing `[bot]` and a trailing parenthesised note, such as `(code review)`, removed.
bare_name(value) := trim_space(regex.replace(trim_suffix(display_name(value), "[bot]"), `\s*\([^)]*\)$`, ""))

is_agent(value) if email_of(value) in agent_emails

is_agent(value) if {
	some parts in regex.find_all_string_submatch_n(
		`^(?:\d+\+)?([^@]+?)(?:\[bot\])?@users\.noreply\.github\.com$`,
		email_of(value),
		1,
	)
	parts[1] in agent_logins
}

is_agent(value) if bare_name(value) in agent_names

is_agent(value) if {
	some pattern in agent_name_patterns
	regex.match(pattern, display_name(value))
}

# No commit on the branch may credit a coding agent anywhere it names a person.
deny contains msg if {
	some identity in identities
	is_agent(identity.value)
	msg := sprintf(agent_msg, [identity.commit, identity.value, identity.where])
}
