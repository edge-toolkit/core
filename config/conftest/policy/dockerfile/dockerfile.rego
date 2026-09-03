# Cross-checks for Dockerfile.nanoserver against the .mise/config*.toml pins, run with `--namespace dockerfile`.
# Evaluated over the Dockerfile plus the .mise/config*.toml files combined (--combine, auto-detected parsers). Two
# rules:
#   1. version drift -- the Dockerfile hard-codes mise install-dir paths (LLVMBIN, the busybox shell, the python dir
#      on PATH) that embed a tool's pinned version; those must match the [tools] pins (reuses mise.rego's matcher).
#   2. MISE_DISABLE_TOOLS -- every pipx: tool in the always-loaded config.toml and in each guest config Nano enables
#      (its ENV MISE_ENV) must be disabled here (pipx can't run on Nano Server), so a newly added pipx tool in any of
#      those can't silently break the Windows build.
package dockerfile

import data.mise

# Collect every string argument of an ENV/RUN instruction in the Dockerfile.
# A Dockerfile's parsed contents is the array of instruction objects; the TOMLs parse to objects.
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

# Collect the comma-separated tools the Dockerfile asks mise to skip.
# The value is usually built from multiple ARGs and composed into the final ENV (a single
# `ENV MISE_DISABLE_TOOLS=...` would be too long, and the no-backslash rule forbids continuing it), so scan every
# ARG/ENV value in the file and take whichever tokens look like a tool name (contain `:`). The `${VAR}` placeholders the
# composing ENV holds get rejected by the same filter -- only the leaf ARG values supply tools.
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

# The guest languages Nano Server enables, read from its `ENV MISE_ENV=<langs>`.
# Only the always-loaded config.toml and these guest configs load on Nano, so only their pipx tools can run there.
# The dockerfile parser stores an ENV's key and value as adjacent Value elements (["MISE_ENV", "dart,java,..."]),
# so find the "MISE_ENV" key and split the element that follows it.
nano_langs contains lang if {
	some file in input
	is_array(file.contents)
	some instr in file.contents
	instr.Cmd == "env"
	some i
	instr.Value[i] == "MISE_ENV"
	some lang in split(instr.Value[i + 1], ",")
}

# The pipx tools Nano can try to install come only from config.toml and its enabled guest configs.
# Collect the always-loaded config.toml plus each config.<lang>.toml whose lang is in MISE_ENV.
pipx_scope_path contains path if {
	some file in input
	endswith(file.path, ".mise/config.toml")
	path := file.path
}

pipx_scope_path contains path if {
	some file in input
	some lang in nano_langs
	endswith(file.path, sprintf(".mise/config.%s.toml", [lang]))
	path := file.path
}

# Every pipx:* tool in an in-scope config must be in Dockerfile.nanoserver's MISE_DISABLE_TOOLS.
# In-scope means the always-loaded config.toml plus each guest config Nano enables.
# pipx:* tools can't run on Nano Server (no CPython, and the rustpython-based pipx bootstrap in config.windows.toml's
# preinstall is currently broken with STATUS_DLL_NOT_FOUND / STATUS_ENTRYPOINT_NOT_FOUND on Nano's stripped API set,
# hence the ENABLE_RUSTPYTHON_PIPX_BOOTSTRAP env gate that defaults off). A previous canary carve-out (pipx:cowsay
# stayed ENABLED on Nano so the build exercised the bootstrap end-to-end) was dropped together with the bootstrap;
# re-introduce when the bootstrap is fixed upstream and re-enabled by default.
deny contains msg if {
	some file in input
	file.path in pipx_scope_path
	some name, _ in file.contents.tools
	startswith(name, "pipx:")
	not disabled_tools[name]
	msg := sprintf(
		"%s: pipx tool %q must be in Dockerfile.nanoserver MISE_DISABLE_TOOLS (pipx tool fails on Nano Server)",
		[file.path, name],
	)
}

# RUN heredocs must invoke `bash` and use a QUOTED delimiter (`RUN bash <<'EOF'`). Two-part rule.
#
# 1. bash as the interpreter. BuildKit's default heredoc shell is `/bin/sh`, which on Debian/Ubuntu is dash -- and
#    dash rejects `set -euo pipefail` with `Illegal option -o pipefail` (exit 2). Routing the heredoc body through
#    bash makes the strict-mode line at the top of every body actually work. Per the Dockerfile spec the interpreter
#    goes BEFORE the `<<TAG` opener (not after -- BuildKit treats trailing words as part of the literal command, the
#    inverse `RUN <<EOF bash` form never works).
# 2. Quoted delimiter. With an unquoted `<<EOF` the outer `/bin/sh -c` that wraps the RUN performs `$(...)` command
#    substitution on the body BEFORE handing it to bash. On Fedora that ran `apt-cache` (a Debian-only tool) and
#    aborted with `apt-cache: command not found`; on Debian/Ubuntu it ran `apt-cache` before the script's own
#    `apt-get update` line, so the cache was stale and the lookup returned empty. Quoting (`<<'EOF'`) defers all
#    expansion to bash, which only evaluates the line inside the correct package-manager branch. ARG values needed
#    inside the body are promoted to ENV before the RUN so bash can resolve them from the environment.
#
# The `set -euo pipefail` first-line check itself is a semgrep rule (the Dockerfile parser flattens heredoc bodies out
# of the AST, so conftest can't see them; semgrep operates on the raw file text).
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

deny contains msg if {
	some file in input
	is_array(file.contents)
	some instr in file.contents
	instr.Cmd == "run"
	some value in instr.Value
	some line in split(value, "\n")
	not startswith(trim_space(line), "#")
	regex.match(`\bapt-get\s+install\b`, line)
	not contains(line, "--no-install-recommends")
	msg := sprintf("%s: `apt-get install` must include --no-install-recommends on the same line", [file.path])
}

deny contains msg if {
	some file in input
	is_array(file.contents)
	some instr in file.contents
	instr.Cmd == "run"
	some value in instr.Value
	some line in split(value, "\n")
	not startswith(trim_space(line), "#")
	regex.match(`\bapt(\s|$)`, line)
	msg := sprintf("%s: use `apt-get`, not `apt` (apt's UI is not stable across releases)", [file.path])
}

# config.linux.toml's [bootstrap.packages] rows must enumerate the Dockerfile's package-list ARGs.
# A workstation verifies itself from that table (`mise bootstrap packages status`) while the Docker build installs
# from COMMON_PACKAGES + APT_PACKAGES / DNF_PACKAGES, so the two describe one prerequisite set from two places and
# would otherwise drift apart with nothing to catch it. Compared per manager: apt against COMMON_PACKAGES +
# APT_PACKAGES, dnf against COMMON_PACKAGES + DNF_PACKAGES.
package_arg_names := {"APT_PACKAGES", "COMMON_PACKAGES", "DNF_PACKAGES"}

# Each package-list ARG in the Linux Dockerfile, as a set of package names.
# The parser hands an ARG back as one `NAME="a b c"` string, so split off the name and strip the quotes.
# `endswith(path, "Dockerfile")` keeps this off Dockerfile.nanoserver, which carries no package ARGs.
arg_packages[name] := pkgs if {
	some file in input
	is_array(file.contents)
	endswith(file.path, "Dockerfile")
	some instr in file.contents
	instr.Cmd == "arg"
	some value in instr.Value
	some name in package_arg_names
	startswith(value, concat("", [name, "="]))
	raw := trim(substring(value, count(name) + 1, -1), "\"")
	pkgs := {p | some p in split(raw, " "); p != ""}
}

# The packages each manager declares in config.linux.toml's [bootstrap.packages], keyed by manager.
bootstrap_packages[mgr] := pkgs if {
	some file in input
	endswith(file.path, ".mise/config.linux.toml")
	some mgr in {"apt", "dnf"}
	pkgs := {p |
		some key, _ in file.contents.bootstrap.packages
		startswith(key, concat("", [mgr, ":"]))
		p := substring(key, count(mgr) + 1, -1)
	}
}

expected_packages["apt"] := arg_packages.COMMON_PACKAGES | arg_packages.APT_PACKAGES

expected_packages["dnf"] := arg_packages.COMMON_PACKAGES | arg_packages.DNF_PACKAGES

# A renamed or removed ARG would leave the comparison undefined, which reads as a pass; fail instead.
deny contains msg if {
	some name in package_arg_names
	not arg_packages[name]
	msg := sprintf("Dockerfile: ARG %s not found -- the [bootstrap.packages] cross-check cannot run", [name])
}

deny contains msg if {
	some mgr, want in expected_packages
	missing := want - bootstrap_packages[mgr]
	count(missing) > 0
	msg := sprintf(
		".mise/config.linux.toml: [bootstrap.packages] lacks %q rows for %s, which the Dockerfile ARGs install",
		[mgr, concat(", ", sort(missing))],
	)
}

deny contains msg if {
	some mgr, want in expected_packages
	extra := bootstrap_packages[mgr] - want
	count(extra) > 0
	msg := sprintf(
		".mise/config.linux.toml: [bootstrap.packages] has %q rows for %s that the Dockerfile ARGs do not install",
		[mgr, concat(", ", sort(extra))],
	)
}
