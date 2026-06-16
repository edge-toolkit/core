# .mise/config*.toml policy, evaluated over conftest's `--combine` input (an array
# of {path, contents}). Run with `--namespace mise` (or `--all-namespaces`).
package mise

is_mise(file) if startswith(file.path, ".mise/config")

# A task `run` must be a string, not an array: taplo's reorder_arrays would
# re-sort the commands of an array form and scramble the sequence.
deny contains msg if {
	some file in input
	is_mise(file)
	some name, task in file.contents.tasks
	is_array(task.run)
	msg := sprintf("%s: task %q run must be a string, not an array", [file.path, name])
}

# A multiline `run` must use `shell = "bash -euo pipefail -c"` so a failing
# command fails the task instead of being masked.
deny contains msg if {
	some file in input
	is_mise(file)
	some name, task in file.contents.tasks
	is_string(task.run)
	contains(task.run, "\n")
	not task.shell == "bash -euo pipefail -c"
	msg := sprintf("%s: task %q has a multiline run; set shell = \"bash -euo pipefail -c\"", [file.path, name])
}

# Task descriptions must be single-line (keep them under the 120-char limit).
deny contains msg if {
	some file in input
	is_mise(file)
	some name, task in file.contents.tasks
	is_string(task.description)
	contains(task.description, "\n")
	msg := sprintf("%s: task %q description must be a single line", [file.path, name])
}

# `cargo:` tools build from source; prefer a prebuilt backend. Allowed only when
# either (a) the tool has no prebuilt anywhere -- allowlisted by name below, or
# (b) it is os-scoped to second-tier platforms (linux/arm64, macos/x64), whose
# prebuilt assets release authors often skip. A first-tier platform (linux/x64,
# macos/arm64, windows) must always have a prebuilt; source-builds there are a
# slow surprise on the critical path.
second_tier_platform := {"linux/arm64", "macos/x64"}

allowed_cargo_no_prebuilt := {"cargo:cargo-expand", "cargo:dart-typegen"}

cargo_scoped_to_second_tier(spec) if {
	is_object(spec)
	count(spec.os) > 0
	every p in spec.os {
		second_tier_platform[p]
	}
}

deny contains msg if {
	some file in input
	is_mise(file)
	some name, spec in file.contents.tools
	startswith(name, "cargo:")
	not allowed_cargo_no_prebuilt[name]
	not cargo_scoped_to_second_tier(spec)
	msg := sprintf(
		"%s: tool %q builds from source; use a prebuilt backend or os-scope to {linux/arm64, macos/x64}",
		[file.path, name],
	)
}

# `ubi:` is deprecated; the `http:` backend replaces it.
deny contains msg if {
	some file in input
	is_mise(file)
	some name, _ in file.contents.tools
	startswith(name, "ubi:")
	msg := sprintf("%s: tool %q uses the deprecated ubi backend; use http: instead", [file.path, name])
}

# Tools should work on every OS (CLAUDE.md "Tools must work on every OS"). Any
# os-scoped [tools] entry must be in this list -- a genuinely platform-specific
# tool, a per-platform backend pair that still covers every OS (findutils, ryl),
# or an optional tool that self-skips on the omitted platform (pipx:torch).
allowed_os_scoped_tool := {
	"chromedriver",
	"pipx",
	"pipx:torch",
	"npm:pnpm",
	"pnpm",
	"github:christianhelle/openapi2zig",
	"github:owenlamont/ryl",
	"github:uutils/findutils",
	"cargo:findutils",
	"cargo:ryl",
	"conda:gnupg",
}

deny contains msg if {
	some file in input
	is_mise(file)
	some name, spec in file.contents.tools
	is_object(spec)
	spec.os
	not allowed_os_scoped_tool[name]
	msg := sprintf("%s: tool %q is os-scoped; tools must work on every OS (or allowlist it)", [file.path, name])
}

# A [vars]/[env] value that hard-codes a tool's install path (e.g. the absolute
# linker in config.windows.toml, or the libpython rpath in config.toml) embeds
# the tool's version as a path segment. mise installs a tool to
# `installs/<dir>/<version>`, where <dir> is the tool name with `:` and `/`
# turned into `-`. Those embedded versions must track the `[tools]` pin -- a
# bump that updates the tool but not the var silently points at a missing dir.
# Collect every (install-dir, version) the [tools] tables pin, across all files
# (--combine), since a var in config.<os>.toml can reference a tool pinned in
# config.toml.
tool_versions contains [dir, version] if {
	some file in input
	is_mise(file)
	some name, spec in file.contents.tools
	is_string(spec)
	dir := replace(replace(name, ":", "-"), "/", "-")
	version := spec
}

tool_versions contains [dir, version] if {
	some file in input
	is_mise(file)
	some name, spec in file.contents.tools
	is_object(spec)
	dir := replace(replace(name, ":", "-"), "/", "-")
	version := spec.version
}

# Every [vars]/[env] string, tagged with where it came from for the message.
config_strings contains entry if {
	some file in input
	is_mise(file)
	some kind in ["vars", "env"]
	some key, value in object.get(file.contents, kind, {})
	is_string(value)
	entry := {"path": file.path, "kind": kind, "key": key, "value": value}
}

# An install path embeds a tool's version as the segment right after the tool's
# install dir. Yield every embedded version that isn't a pinned version of that
# tool. The captured segment is restricted to version chars so it stops at the
# next path separator OR a trailing delimiter (a quote, `;`, …) when the path is
# spliced into a larger string (as in Dockerfile ENV/RUN lines). Shared by the
# [vars]/[env] check here and the Dockerfile check (data.mise.version_drift).
version_drift(value) := {drift |
	some [dir, _] in tool_versions
	pattern := sprintf(`(?:^|[\\/}])%s[\\/]([A-Za-z0-9._-]+)`, [dir])
	some m in regex.find_all_string_submatch_n(pattern, value, -1)
	seg := m[1]
	not [dir, seg] in tool_versions
	pinned := concat(", ", sort({v | some [d, v] in tool_versions; d == dir}))
	drift := {"dir": dir, "seg": seg, "pinned": pinned}
}

deny contains msg if {
	some entry in config_strings
	contains(entry.value, "installs")
	some d in version_drift(entry.value)
	msg := sprintf(
		"%s: %s %q embeds %q version %q, but [tools] pins it to %q -- keep them in sync",
		[entry.path, entry.kind, entry.key, d.dir, d.seg, d.pinned],
	)
}

tool_version_str(spec) := spec if is_string(spec)

tool_version_str(spec) := spec.version if is_object(spec)

# python must be pinned to a full version triple (X.Y.Z), not a minor alias:
# mise installs it under a dir named after the request and only symlinks the X.Y
# alias, and that symlink isn't created on the Windows runner -- so the py3_*
# interpreter paths (and the version_drift check above) need the exact patch dir.
deny contains msg if {
	some file in input
	is_mise(file)
	version := tool_version_str(file.contents.tools.python)
	not regex.match(`^[0-9]+\.[0-9]+\.[0-9]+$`, version)
	msg := sprintf("%s: python must be pinned to a full version triple, got %q", [file.path, version])
}

# Linux + macOS preinstall MUST read its prerequisite package list from the
# Dockerfile (COMMON_PACKAGES + APT_PACKAGES / DNF_PACKAGES ARGs). The
# Dockerfile is the single source of truth: a capability-based or hardcoded
# check would diverge from it silently. Guard by requiring the preinstall
# task body to reference both COMMON_PACKAGES and APT_PACKAGES somewhere
# (env var or rg-parse of the file).
preinstall_must_reference_apt_packages := {
	".mise/config.linux.toml",
	".mise/config.macos.toml",
}

required_package_args := {"APT_PACKAGES", "COMMON_PACKAGES"}

deny contains msg if {
	some file in input
	preinstall_must_reference_apt_packages[file.path]
	task := file.contents.tasks.preinstall
	some required in required_package_args
	not contains(task.run, required)
	msg := sprintf(
		"%s: tasks.preinstall.run must reference %s (Dockerfile is the single source of truth)",
		[file.path, required],
	)
}
