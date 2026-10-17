# Unit tests for the declared-stage rule over published scenario Dockerfiles, run by `conftest verify`.
#
# The rule reads stage names out of parsed instructions, and a parse shape it stops recognising fails open: every
# Dockerfile then passes while nothing is checked. A reference known to be undeclared is what separates the two.
package dockerfile_stages_test

import data.dockerfile_stages

# An instruction shaped the way conftest's dockerfile parser emits one.
instr(cmd, flags, value, stage) := {"Cmd": cmd, "Flags": flags, "Value": value, "Stage": stage}

released := instr("from", [], ["docker.io/library/node:22-bookworm-slim", "AS", "released"], 0)

hub := instr("from", [], ["ghcr.io/edge-toolkit/core/et-ws-server:latest"], 1)

test_a_copy_from_a_declared_earlier_stage_passes if {
	msgs := dockerfile_stages.deny with input as [
		released,
		hub,
		instr("copy", ["--from=released", "--chown=10001:10001"], ["/staged/app", "/app"], 1),
	]
	count(msgs) == 0
}

test_a_copy_from_an_undeclared_stage_is_denied if {
	msgs := dockerfile_stages.deny with input as [
		hub,
		instr("copy", ["--from=deps"], ["/staged", "/app"], 0),
	]
	count(msgs) == 1
}

test_a_stage_declared_after_its_use_is_denied if {
	msgs := dockerfile_stages.deny with input as [
		instr("from", [], ["ghcr.io/edge-toolkit/core/et-ws-server:latest"], 0),
		instr("copy", ["--from=released"], ["/staged", "/app"], 0),
		instr("from", [], ["docker.io/library/node:22-bookworm-slim", "AS", "released"], 1),
	]
	count(msgs) == 1
}

test_a_mount_from_an_undeclared_stage_is_denied if {
	msgs := dockerfile_stages.deny with input as [
		hub,
		instr("run", ["--mount=type=cache,from=deps,target=/cache"], ["true"], 0),
	]
	count(msgs) == 1
}

test_a_mount_with_no_from_is_ignored if {
	msgs := dockerfile_stages.deny with input as [
		released,
		instr("run", ["--mount=type=secret,id=gh_token,env=GITHUB_TOKEN"], ["true"], 0),
	]
	count(msgs) == 0
}

test_stage_names_compare_without_case if {
	msgs := dockerfile_stages.deny with input as [
		instr("from", [], ["docker.io/library/node:22-bookworm-slim", "as", "Released"], 0),
		hub,
		instr("copy", ["--from=released"], ["/staged", "/app"], 1),
	]
	count(msgs) == 0
}
