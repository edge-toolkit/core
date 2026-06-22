# GitHub Actions workflow rules that need the file path (not just the parsed body), so they run under `--combine`:
# conftest hands every file in as an array of `{path, contents}`. Per-file rules live in `gha.rego`.
package gha_combined

# Every workflow must trigger on pull_request: PR CI runs it when relevant code changes. Skip the file-name
# allowlist here; the path-self-reference rule below ensures workflows that DO scope by paths still re-run when their
# own YAML is touched.
deny contains msg if {
	some file in input
	endswith(file.path, ".yaml")
	startswith(file.path, ".github/workflows/")
	not file.contents.on.pull_request

	# `on:` parses to an object; YAML's `on:` without a body is `null`, not a key.
	msg := sprintf("%s: workflow must trigger on `pull_request`", [file.path])
}

# Every workflow must support manual workflow_dispatch: re-running a failed/flaky run shouldn't need a no-op commit.
# `workflow_dispatch:` with no body parses to `null`, so check for the key directly.
deny contains msg if {
	some file in input
	endswith(file.path, ".yaml")
	startswith(file.path, ".github/workflows/")
	not "workflow_dispatch" in object.keys(file.contents.on)
	msg := sprintf("%s: workflow must include `workflow_dispatch:` (manual rerun)", [file.path])
}

# When a workflow scopes its pull_request trigger by `paths`, the workflow's own YAML must be in that list --
# otherwise edits to the workflow itself don't re-run it on the PR that introduces them, and the change ships
# unverified. Workflows without `paths` (always-run on every PR) are fine and need no entry.
deny contains msg if {
	some file in input
	endswith(file.path, ".yaml")
	startswith(file.path, ".github/workflows/")
	paths := file.contents.on.pull_request.paths
	is_array(paths)
	not file.path in paths
	msg := sprintf(
		"%s: `on.pull_request.paths` must include the workflow itself (%q) so edits to it re-run",
		[file.path, file.path],
	)
}
