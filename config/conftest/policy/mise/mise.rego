# .mise/config*.toml policy.
# Evaluated over conftest's `--combine` input (an array of {path, contents}). Run with `--namespace mise` (or
# `--all-namespaces`).
package mise

# The separator replace keeps this true on Windows-local runs.
# conftest reports native backslash paths when handed the `.mise` DIRECTORY (as conftest-check-yaml's
# gha_mise pass and conftest-check-dockerfile do), which would otherwise silently empty every cross-check
# derived from this predicate there; CI's Linux runs were unaffected.
is_mise(file) if startswith(replace(file.path, "\\", "/"), ".mise/config")

# A task `run` must be a string, not an array.
# taplo's reorder_arrays would re-sort the commands of an array form and scramble the sequence.
deny contains msg if {
	some file in input
	is_mise(file)
	some name, task in file.contents.tasks
	is_array(task.run)
	msg := sprintf("%s: task %q run must be a string, not an array", [file.path, name])
}

# A multiline `run` must use `shell = "{{ vars.task_shell }}"`.
# This makes a failing command fail the task instead of being masked. The xtrace variant (`task_shell_trace`) is
# also accepted: it prints every command as it runs, used on the Windows OS-specific preinstall where full transcripts
# matter for diagnosing busybox-ash + path-mangling failures. `task_shell_plain` is deliberately absent -- it carries
# no `-e`, which is exactly the masking this rule exists to prevent. Conftest reads the TOML unrendered, so these are
# the literal template strings rather than the shell command lines they expand to.
allowed_run_shells := {"{{ vars.task_shell }}", "{{ vars.task_shell_trace }}"}

deny contains msg if {
	some file in input
	is_mise(file)
	some name, task in file.contents.tasks
	is_string(task.run)
	contains(task.run, "\n")
	not task.shell in allowed_run_shells
	msg := sprintf(
		"%s: task %q has a multiline run; set shell = \"{{ vars.task_shell }}\" (or task_shell_trace for xtrace)",
		[file.path, name],
	)
}

# Task descriptions must be single-line.
deny contains msg if {
	some file in input
	is_mise(file)
	some name, task in file.contents.tasks
	is_string(task.description)
	contains(task.description, "\n")
	msg := sprintf("%s: task %q description must be a single line", [file.path, name])
}

# `compgen` is a bash builtin that busybox-w32 ash (Nano Server's shell) does not provide.
# A task using it fails on Nano with the literal error `<compgen>: not found` (verified in the build-rp-native task).
# The whole run body is checked, comments included -- compgen-narrating prose belongs in a TOML comment on
# the task, not inside the body. config.maint.toml is exempt: maintainer-only, never runs on Nano.
# POSIX-portable alternatives include `[ -e "$prefix/bin/foo" ] || [ -e "$prefix/bin/foo.exe" ]` for an
# extension-agnostic existence check, and `for f in "$prefix/bin/foo"*` to walk a literal glob (no-match leaves the
# literal as the loop var).
deny contains msg if {
	some file in input
	is_mise(file)
	replace(file.path, "\\", "/") != ".mise/config.maint.toml"
	some name, task in file.contents.tasks
	is_string(task.run)
	regex.match(`\bcompgen\b`, task.run)
	msg := sprintf(
		"%s: task %q uses `compgen`, which is bash-only -- busybox ash (Nano) errors `compgen: not found`",
		[file.path, name],
	)
}

# A task `run` must not embed version-like text (an `X.Y.Z` literal), comments included.
# A version baked into a run body silently goes stale when the pin it mirrors is bumped -- version_drift
# only cross-checks [vars]/[env] values, never run strings. Keep the versioned fragment in a [vars] entry
# beside the [tools] pin it tracks and template it into the run with `{{ vars.<name> }}`; version-narrating
# prose belongs in a TOML comment on the task, not inside the body.
# config.maint.toml is exempt: its maintainer-only investigation/publish tasks narrate and probe specific
# upstream versions by design.
deny contains msg if {
	some file in input
	is_mise(file)
	replace(file.path, "\\", "/") != ".mise/config.maint.toml"
	some name, task in file.contents.tasks
	is_string(task.run)
	some m in regex.find_all_string_submatch_n(`[0-9]+\.[0-9]+\.[0-9]+`, task.run, -1)
	msg := sprintf(
		"%s: task %q run embeds version-like text %q -- move it into a [vars] entry beside the pin it tracks",
		[file.path, name, m[0]],
	)
}

# `cargo:` tools may build from source; prefer a prebuilt backend.
# Allowed only when either (a) allowlisted by name below -- cargo-binstall fetches a prebuilt there (e.g. a
# cargo-quickinstall release, routed via an `install_env` CARGO_BUILD_TARGET override), or the tool has no prebuilt
# anywhere and genuinely source-builds; or (b) it is os-scoped to second-tier platforms (linux/arm64, macos/x64),
# whose prebuilt assets release authors often skip. A first-tier platform (linux/x64, macos/arm64, windows) must
# always have a prebuilt; source-builds there are a slow surprise on the critical path.
#
# config.maint.toml is exempted: it's only loaded with MISE_ENV=maint by a maintainer running one-off publish tasks,
# not by CI. A slow cargo-source install on a workstation when refreshing the HF mirror is fine.
second_tier_platform := {"linux/arm64", "macos/x64"}

# cargo:action-validator has no aqua/github Windows build, so config.windows.toml installs it via cargo.
# cargo-binstall pulls the cargo-quickinstall x86_64-pc-windows-msvc prebuilt (verified present), routed via
# the msvc install_env there -- a prebuilt fetch, not a source build.
allowed_cargo_no_prebuilt := {
	"cargo:action-validator",
	"cargo:cargo-expand",
	"cargo:dart-typegen",
	"cargo:open",
	"cargo:wasm-opt",
}

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
	not file.path == ".mise/config.maint.toml"
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

# A new `http:` tool must not point at a forge release URL; use the `github:` backend for those.
# `http:` takes a hardcoded URL, so it resolves no versions and verifies nothing: a version bump means editing
# every per-platform URL by hand, and the download arrives with neither the release metadata nor the artifact
# attestation the `github:` backend checks. Naming the repo instead keeps version resolution and verification.
#
# The entries below predate the rule. Two groups, kept apart because only one is a candidate for removal: the
# edge-toolkit/core ones are this repo's own upstream-cache mirrors, where a pinned URL plus the `checksum` the
# checksums policy enforces is the documented pattern and `github:` cannot express what is being fetched. The
# third-party ones are the ones to migrate to `github:` as they come up for maintenance.
allowed_http_forge_url := {
	# This repo's own upstream-cache mirror releases -- the documented pattern, not migration candidates.
	"http:augeas",
	"http:dart-typegen",
	"http:et-rp",
	"http:gnupg-w32",
	"http:rp-wasm",
	# Third-party forge releases predating the rule; migrate to `github:` when each is next touched.
	"http:oxfmt",
	"http:oxlint",
	"http:pyodide",
	"http:rustfs",
	"http:webr",
}

forge_url(url) if contains(url, "github.com")

forge_url(url) if contains(url, "gitlab.com")

http_forge_msg := "%s: tool %q fetches %s over http:; use the github: backend so versions resolve and artifacts verify"

deny contains msg if {
	some file in input
	is_mise(file)
	some name, tool in file.contents.tools
	startswith(name, "http:")
	not name in allowed_http_forge_url
	forge_url(tool.url)
	msg := sprintf(http_forge_msg, [file.path, name, tool.url])
}

deny contains msg if {
	some file in input
	is_mise(file)
	some name, tool in file.contents.tools
	startswith(name, "http:")
	not name in allowed_http_forge_url
	some platform in tool.platforms
	forge_url(platform.url)
	msg := sprintf(http_forge_msg, [file.path, name, platform.url])
}

# Tools should work on every OS (CLAUDE.md "Tools must work on every OS").
# Any os-scoped [tools] entry must be in this list -- a genuinely platform-specific tool, a per-platform backend pair
# that still covers every OS (findutils, ryl), or an optional tool that self-skips on the omitted platform (pipx:torch).
allowed_os_scoped_tool := {
	# action-validator (aqua) has no Windows build, so it is os-scoped off Windows.
	# config.windows.toml installs cargo:action-validator there instead (cargo-quickinstall msvc prebuilt).
	"action-validator",
	"http:chromedriver",
	"pipx",
	"pipx:torch",
	"npm:pnpm",
	"pnpm",
	"github:christianhelle/openapi2zig",
	"github:owenlamont/ryl",
	"github:uutils/findutils",
	# macmon is an Apple Silicon monitor with no Linux/Windows build, so the o2-macmon task scopes it to macOS.
	"github:vladkens/macmon",
	# nvidia_gpu_exporter is the Linux NVIDIA GPU path for o2-nvidia; Windows uses windows_exporter, macOS macmon.
	"github:utkuozdemir/nvidia_gpu_exporter",
	# windows_exporter is a Windows-only host/GPU Prometheus exporter, so the o2-winmetrics task scopes it to Windows.
	"github:prometheus-community/windows_exporter",
	"cargo:findutils",
	# vector is the OTLP store-and-forward relay, covered by two per-platform entries.
	# The aqua prebuilt covers every platform upstream ships (no darwin/amd64 asset exists); a cargo source
	# build from the same git tag covers macos/x64. Between the two every platform is covered.
	"aqua:vectordotdev/vector",
	"cargo:vectordotdev/vector",
	# rustfs is the S3 server the storage backend test runs against, covered by two per-platform entries.
	# http: names an upstream asset for every platform that ships one; a cargo source build from the same git
	# tag covers macos/x64, which has no prebuilt at all. Between the two every platform is covered.
	"cargo:rustfs/rustfs",
	"http:rustfs",
	"cargo:ryl",
	# http:et-rp is os-scoped to only those platforms whose tarball is already in the rp-v<N> release.
	# Add a platform by dispatching the upstream-cache.yaml workflow on that host.
	"http:et-rp",
	"conda:gnupg",
	# cargo:cargo-expand: gnullvm source-build fails, so os-scoped off Windows (msvc override in config.windows.toml).
	"cargo:cargo-expand",
	# cargo:dart-typegen is os-scoped to non-Windows because the gnullvm rust host fails on Windows source-builds.
	# It trips `error[E0463]: can't find crate for 'core'`; coverage is preserved via http:dart-typegen in
	# config.windows.toml, which serves the upstream-cache prebuilt.
	"cargo:dart-typegen",
	# cargo:wasm-opt: gnullvm source-build fails, so os-scoped off Windows (msvc override in config.windows.toml).
	"cargo:wasm-opt",
	# winlibs mingw-w64 GCC: the toolchain for the x86_64-pc-windows-gnu target (config.mingw.toml).
	# Upstream ships Windows-only zips, and the env that installs it is itself Windows-only.
	"github:brechtsanders/winlibs_mingw",
	# github:mstorsjo/llvm-mingw: its clang-tidy analyzes the Windows-only mingw-shim C.
	# config.zig.toml os-scopes it to linux/macos (the check hosts that lack it); on Windows config.windows.toml's
	# own llvm-mingw is reused via auto_env.
	"github:mstorsjo/llvm-mingw",
	# conda:clang: a wasm-capable LLVM clang for the wasm-coverage builds (minicov's C profiler runtime).
	# Scoped to linux/macos in config.coverage.toml -- the coverage workflow runs only there, and Windows
	# has no coverage lane.
	"conda:clang",
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

# A [vars]/[env] value that hard-codes a tool's install path embeds the tool's version as a path segment.
# Examples are the absolute linker in config.windows.toml, or the libpython rpath in config.toml. mise installs a tool
# to `installs/<dir>/<version>`, where <dir> is the tool name with `:` and `/` turned into `-`. Those embedded versions
# must track the `[tools]` pin -- a bump that updates the tool but not the var silently points at a missing dir.
# Collect every (install-dir, version) the [tools] tables pin, across all files (--combine), since a var in
# config.<os>.toml can reference a tool pinned in config.toml.
# mise names the install dir after the tool with `:` and `/` flattened to `-`, and (observed on the github
# backend) `_` flattened too: winlibs_mingw lands under github-brechtsanders-winlibs-mingw. Register both
# spellings so a [vars]/[env] path is checked against whichever the backend actually produces.
install_dirs(name) := {base, replace(base, "_", "-")} if {
	base := replace(replace(name, ":", "-"), "/", "-")
}

tool_versions contains [dir, version] if {
	some file in input
	is_mise(file)
	some name, version in file.contents.tools
	is_string(version)
	some dir in install_dirs(name)
}

tool_versions contains [dir, version] if {
	some file in input
	is_mise(file)
	some name, spec in file.contents.tools
	is_object(spec)
	some dir in install_dirs(name)
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

# An install path embeds a tool's version as the segment right after the tool's install dir.
# Yield every embedded version that isn't a pinned version of that tool. The captured segment is restricted to version
# chars so it stops at the next path separator OR a trailing delimiter (a quote, `;`, ...) when the path is spliced into
# a larger string (as in Dockerfile ENV/RUN lines). Shared by the [vars]/[env] check here and the Dockerfile check
# (data.mise.version_drift).
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

# python must be pinned to a full version triple (X.Y.Z), not a minor alias.
# mise installs it under a dir named after the request and only symlinks the X.Y alias, and that symlink isn't created
# on the Windows runner -- so the py3_* interpreter paths (and the version_drift check above) need the exact patch dir.
deny contains msg if {
	some file in input
	is_mise(file)
	version := tool_version_str(file.contents.tools.python)
	not regex.match(`^[0-9]+\.[0-9]+\.[0-9]+$`, version)
	msg := sprintf("%s: python must be pinned to a full version triple, got %q", [file.path, version])
}

# Linux + macOS preinstall MUST read its prerequisite package list from the Dockerfile.
# The Dockerfile's ARGs are the single source of truth: a capability-based or hardcoded check would diverge from them
# silently. Guard by requiring the preinstall task body to name the ARGs it has to parse. macOS follows the apt naming,
# so it needs COMMON_PACKAGES + APT_PACKAGES. Linux verifies apt and dnf hosts from [bootstrap.packages] instead, and
# the dockerfile policy holds that table to these same ARGs; only its Azure Linux / openSUSE arm still parses the file,
# for which the RPM-family list applies.
required_package_args := {
	".mise/config.linux.toml": {"COMMON_PACKAGES", "DNF_PACKAGES"},
	".mise/config.macos.toml": {"APT_PACKAGES", "COMMON_PACKAGES"},
}

deny contains msg if {
	some file in input
	some required in required_package_args[file.path]
	task := file.contents.tasks.preinstall
	not contains(task.run, required)
	msg := sprintf(
		"%s: tasks.preinstall.run must reference %s (Dockerfile is the single source of truth)",
		[file.path, required],
	)
}
