# Cross-checks for Dockerfile.nanoserver, evaluated over the Dockerfile plus the
# .mise/config*.toml files combined (--combine, auto-detected parsers). Two rules:
#   1. version drift -- the Dockerfile hard-codes mise install-dir paths (LLVMBIN,
#      the busybox shell, the python dir on PATH) that embed a tool's pinned
#      version; those must match the [tools] pins (reuses mise.rego's matcher).
#   2. MISE_DISABLE_TOOLS -- every pipx: tool in the always-loaded config.toml
#      must be disabled here (pipx can't run on Nano Server), so a newly added
#      pipx tool can't silently break the Windows build.
# Run with `--namespace dockerfile`.
package dockerfile

import data.mise

# Every string argument of an ENV/RUN instruction in the Dockerfile (its parsed
# contents is the array of instruction objects; the TOMLs parse to objects).
docker_strings contains entry if {
	some file in input
	is_array(file.contents)
	some instr in file.contents
	instr.Cmd in {"env", "run"}
	some value in instr.Value
	is_string(value)
	entry := {"path": file.path, "value": value}
}

deny contains msg if {
	some entry in docker_strings
	contains(entry.value, "installs")
	some d in mise.version_drift(entry.value)
	msg := sprintf(
		"%s: hard-codes %q version %q, but [tools] pins it to %q -- keep them in sync",
		[entry.path, d.dir, d.seg, d.pinned],
	)
}

# The comma-separated tools the Dockerfile asks mise to skip. The value is
# usually built from multiple ARGs and composed into the final ENV (the
# 120-char limit plus the project's no-backslash rule make a single
# `ENV MISE_DISABLE_TOOLS=...` line impractical), so scan every ARG/ENV
# value in the file and take whichever tokens look like a tool name
# (contain `:`). The `${VAR}` placeholders the composing ENV holds get
# rejected by the same filter — only the leaf ARG values supply tools.
disabled_tools contains tool if {
	some file in input
	is_array(file.contents)
	some instr in file.contents
	instr.Cmd in {"arg", "env"}
	some value in instr.Value
	some token in split(value, ",")
	contains(token, ":")
	tool := trim_space(token)
}

# pipx:* tools can't run on Nano Server (pipx/platformdirs can't import there), so
# every pipx: tool in the always-loaded config.toml must be in MISE_DISABLE_TOOLS
# -- otherwise the nano build tries to install it and fails.
deny contains msg if {
	some file in input
	endswith(file.path, ".mise/config.toml")
	some name, _ in file.contents.tools
	startswith(name, "pipx:")
	not disabled_tools[name]
	msg := sprintf(
		"%s: pipx tool %q must be in Dockerfile.nanoserver MISE_DISABLE_TOOLS (pipx fails on Nano Server)",
		[file.path, name],
	)
}

# Every RUN heredoc body must begin with `set -euo pipefail`. The Dockerfile
# parser surfaces a heredoc's first-line value via instr.Heredocs[].Content
# (without the EOF terminator), or for non-heredoc RUNs as instr.Value[0].
# Mirrors the gha.rego rule that enforces the same on multi-line `run:` in
# workflow YAML -- one strict-mode declaration at the top of every script
# body so future maintainers don't have to remember which RUN inherits
# which shell flags.
run_heredocs contains body if {
	some file in input
	is_array(file.contents)
	some instr in file.contents
	instr.Cmd == "run"
	some heredoc in instr.Heredocs
	body := {"path": file.path, "name": object.get(heredoc, "Name", ""), "content": object.get(heredoc, "Content", "")}
}

deny contains msg if {
	some body in run_heredocs
	not startswith(body.content, "set -euo pipefail\n")
	msg := sprintf(
		"%s: RUN <<%s heredoc must begin with `set -euo pipefail` as the first line",
		[body.path, body.name],
	)
}
