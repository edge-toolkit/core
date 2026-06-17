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

# RUN heredocs must invoke `bash` and use a QUOTED delimiter
# (`RUN bash <<'EOF'`). Two-part rule:
#
# 1. bash as the interpreter. BuildKit's default heredoc shell is
#    `/bin/sh`, which on Debian/Ubuntu is dash -- and dash rejects
#    `set -euo pipefail` with `Illegal option -o pipefail` (exit 2).
#    Routing the heredoc body through bash makes the strict-mode line
#    at the top of every body actually work. Per the Dockerfile spec
#    the interpreter goes BEFORE the `<<TAG` opener (not after --
#    BuildKit treats trailing words as part of the literal command,
#    the inverse `RUN <<EOF bash` form never works).
# 2. Quoted delimiter. With an unquoted `<<EOF` the outer `/bin/sh -c`
#    that wraps the RUN performs `$(...)` command substitution on the
#    body BEFORE handing it to bash. On Fedora that ran `apt-cache` (a
#    Debian-only tool) and aborted with `apt-cache: command not found`;
#    on Debian/Ubuntu it ran `apt-cache` before the script's own
#    `apt-get update` line, so the cache was stale and the lookup
#    returned empty. Quoting (`<<'EOF'`) defers all expansion to bash,
#    which only evaluates the line inside the correct package-manager
#    branch. ARG values needed inside the body are promoted to ENV
#    before the RUN so bash can resolve them from the environment.
#
# The `set -euo pipefail` first-line check itself is a semgrep rule
# (the Dockerfile parser flattens heredoc bodies out of the AST, so
# conftest can't see them; semgrep operates on the raw file text).
deny contains msg if {
	some file in input
	is_array(file.contents)
	some instr in file.contents
	instr.Cmd == "run"
	some value in instr.Value
	contains(value, "<<")
	not startswith(value, "bash ")
	msg := sprintf(
		"%s: RUN heredoc `%s` must use bash as interpreter (write `RUN bash <<'EOF'`)",
		[file.path, value],
	)
}

deny contains msg if {
	some file in input
	is_array(file.contents)
	some instr in file.contents
	instr.Cmd == "run"
	some value in instr.Value
	contains(value, "<<")
	startswith(value, "bash ")
	not contains(value, "<<'")
	not contains(value, "<<\"")
	msg := sprintf(
		"%s: RUN heredoc `%s` must use a quoted delimiter (write `<<'EOF'`) to defer $-expansion to bash",
		[file.path, value],
	)
}
