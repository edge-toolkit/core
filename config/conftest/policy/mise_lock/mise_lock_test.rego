package mise_lock_test

import data.mise_lock

allowed := {"github:example/allowed": "GLIBC_2.17"}

# One lockfile holding a single tool locked to the given Linux URL.
lock(name, url) := [{
	"path": ".mise/mise.lock",
	"contents": {"tools": {name: [{"version": "1", "platforms.linux-x64": {"url": url}}]}},
}]

denied_for(msgs, name) if {
	some msg in msgs
	contains(msg, $"{name} locks a glibc build for linux-x64")
}

test_musl_build_is_allowed if {
	not denied_for(mise_lock.deny, "github:example/tool") with input as lock(
		"github:example/tool",
		"https://example.test/tool-x86_64-unknown-linux-musl.tar.gz",
	)
		with mise_lock.gnu_allowed as {}
}

test_unlisted_gnu_build_is_denied if {
	denied_for(mise_lock.deny, "github:example/tool") with input as lock(
		"github:example/tool",
		"https://example.test/tool-x86_64-unknown-linux-gnu.tar.gz",
	)
		with mise_lock.gnu_allowed as allowed
}

test_allowlisted_gnu_build_is_allowed if {
	msgs := mise_lock.deny with input as lock(
		"github:example/allowed",
		"https://example.test/allowed-x86_64-unknown-linux-gnu.zip",
	)
		with mise_lock.gnu_allowed as allowed
	count(msgs) == 0
}

test_stale_allowlist_entry_is_denied if {
	some msg in mise_lock.deny with input as lock(
		"github:example/tool",
		"https://example.test/tool-x86_64-unknown-linux-musl.tar.gz",
	)
		with mise_lock.gnu_allowed as allowed
	contains(msg, "gnu_allowed lists github:example/allowed")
}
