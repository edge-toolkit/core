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

# The comma-separated tools in the Dockerfile's `ENV MISE_DISABLE_TOOLS=...`.
disabled_tools contains tool if {
	some file in input
	is_array(file.contents)
	some instr in file.contents
	instr.Cmd == "env"
	instr.Value[0] == "MISE_DISABLE_TOOLS"
	some tool in split(instr.Value[1], ",")
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
