# .mise/mise*.lock policy: a Linux tool locks its musl build unless it is allowlisted for a glibc one.
# Evaluated over conftest's `--combine` input (an array of {path, contents}). Run with `--namespace mise_lock`.
#
# A glibc build starts only where the host's glibc is at least as new as the one it was linked against, and the
# oldest image bases this repo builds on ship glibc 2.35 (ubuntu:22.04) and 2.36 (debian:bookworm). mise picks the
# gnu asset for a glibc platform whenever a release publishes both, so a lock refresh can silently swap a portable
# musl build for one those images cannot run: uutils coreutils did exactly that, and every `coreutils` call on
# both images failed with it.
package mise_lock

is_lock(file) if regex.match(`^\.mise/mise(\.[a-z]+)?\.lock$`, replace(file.path, "\\", "/"))

# Tools allowed to lock a glibc Linux build, each with the glibc floor it was measured to need.
#
# The floor is the highest GLIBC_ symbol version the tool's binaries reference. A floor above 2.35 is a build the
# ubuntu:22.04 image cannot start; such an entry stays only because no musl build exists for it.
gnu_allowed := {
	"aqua:01mf02/jaq": "GLIBC_2.34",
	"aqua:vectordotdev/vector": "GLIBC_2.28",
	"ast-grep": "GLIBC_2.34",
	"cargo-binstall": "GLIBC_2.17",
	"dprint": "GLIBC_2.17",
	"github:caldempsey/parfit": "GLIBC_2.34",
	"github:grok-rs/waitup": "GLIBC_2.39 on x64, above the 2.35 floor",
	"github:nextest-rs/nextest": "GLIBC_2.27",
	"github:owenlamont/ryl": "GLIBC_2.34",
	"github:uutils/findutils": "GLIBC_2.29",
	"github:wasm-bindgen/wasm-bindgen": "GLIBC_2.39 on arm64, above the 2.35 floor",
	"http:augeas": "GLIBC_2.38, above the 2.35 floor; the upstream-cache mirror builds no musl variant",
	"http:et-rp": "GLIBC_2.38, above the 2.35 floor; the upstream-cache mirror builds no musl variant",
	"http:oxfmt": "GLIBC_2.18",
	"http:oxlint": "GLIBC_2.18",
	"python": "GLIBC_2.17",
	"ripgrep": "GLIBC_2.18",
	"ruff": "GLIBC_2.17",
	"uv": "GLIBC_2.28",
	"watchexec": "GLIBC_2.39 on arm64, above the 2.35 floor",
	"zizmor": "GLIBC_2.34",
}

# Every (lockfile, tool, platform) whose locked Linux asset is a glibc build.
gnu_locks contains [file.path, name, platform] if {
	some file in input
	is_lock(file)
	some name, entries in file.contents.tools
	some entry in entries
	some key, locked in entry
	startswith(key, "platforms.linux-")
	regex.match(`linux[-_]gnu`, locked.url)
	platform := trim_prefix(key, "platforms.")
}

musl_hint := "select its musl asset (a github backend `asset_pattern` per Linux platform), or allowlist its floor"

# A tool locks a glibc Linux build without an allowlist entry.
deny contains msg if {
	some [path, name, platform] in gnu_locks
	not gnu_allowed[name]
	msg := $"{path}: {name} locks a glibc build for {platform}; {musl_hint}"
}

# An allowlist entry names a tool that no longer locks a glibc build anywhere.
deny contains msg if {
	some name, reason in gnu_allowed
	not name in gnu_locked_names
	msg := $"mise_lock.rego: gnu_allowed lists {name} ({reason}), which no longer locks a glibc build; drop the entry"
}

gnu_locked_names contains name if {
	some [_, name, _] in gnu_locks
}
