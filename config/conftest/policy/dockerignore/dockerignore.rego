# The Docker build context, held to keeping every top-level Markdown file out of it.
# Run with `--namespace dockerignore` over .dockerignore, `--combine`d and parsed with `ignore`, which hands the
# file over as its lines in order; the `Original` field of each is the line's text.
#
# A top-level Markdown file is written for a reader of the repository and is never an input to an image build, so
# copying one into the context only means that editing it re-runs every layer from `COPY . .` on. A Docker pattern
# is anchored at the context root, so `*.md` covers the top level and nothing below it, where each crate's README is
# an input to its rustdoc. Docker applies the last pattern a path matches, so what counts is that no negation after
# the last `*.md` can name a top-level Markdown file; one before it is overridden and harmless.
package dockerignore

import data.paths

top_level_markdown := "*.md"

lines := [line.Original |
	some file in paths.files
	endswith(file.path, ".dockerignore")
	some line in file.contents
]

excludes := [i | some i, text in lines; text == top_level_markdown]

last_exclude := max(excludes) if count(excludes) > 0

# A negated pattern's target, stripped of the anchors that still let it reach the top level.
# Those are a leading `/`, and any `**/`, which matches zero directories as well as many.
negated_name(text) := regex.replace(trim_prefix(text, "!"), `^/?(\*\*/)*`, "")

# Whether a negated pattern can name a top-level Markdown file.
# A name with no `/` left is a top-level pattern, and it can name a Markdown file if it ends in `.md`, or in a
# wildcard or character class that `.md` could complete.
reaches_top_level_markdown(text) if {
	name := negated_name(text)
	not contains(name, "/")
	could_name_markdown(name)
}

could_name_markdown(name) if endswith(name, ".md")

could_name_markdown(name) if regex.match(`[*?\]]\.?m?d?$`, name)

# .dockerignore must exclude every top-level Markdown file.
deny contains msg if {
	count(excludes) == 0
	msg := $".dockerignore: no `{top_level_markdown}` line, so top-level Markdown reaches the Docker build context"
}

# Nothing after the last `*.md` may bring a top-level Markdown file back into the context.
deny contains msg if {
	some i, text in lines
	i > last_exclude
	startswith(text, "!")
	reaches_top_level_markdown(text)
	msg := $".dockerignore: `{text}` follows the last `{top_level_markdown}` and brings top-level Markdown back"
}
