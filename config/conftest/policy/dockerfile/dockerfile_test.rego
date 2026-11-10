# Unit tests for the Dockerfile-versus-mise cross-checks, run by `conftest verify`.
#
# The real Dockerfiles already comply, so the real task cannot tell a clean repo from a rule that stopped matching.
# Each test starts from a small repo that passes every rule and changes one thing, so a test that expects a denial
# also proves the clean baseline really is clean.
package dockerfile_test

import data.dockerfile

instr(cmd, values) := {"Cmd": cmd, "Value": values}

root_instrs := [
	instr("arg", [`COMMON_PACKAGES="bash curl"`]),
	instr("arg", [`APT_PACKAGES="g++"`]),
	instr("arg", [`DNF_PACKAGES="gcc-c++"`]),
	instr("arg", ["MISE_VERSION=v2026.10.15"]),
	instr("arg", ["MISE_ENV=dart,python"]),
	instr("env", ["MISE_ENV", "${MISE_ENV}"]),
	instr("run", ["bash <<'EOF'"]),
]

nano_instrs := [
	instr("arg", ["MISE_DT_BASE=pipx:cowsay,cargo:open"]),
	instr("env", ["MISE_DISABLE_TOOLS", "${MISE_DT_BASE}"]),
	instr("env", ["MISE_ENV", "dart"]),
	instr("env", ["PATH", `C:\i\http-busybox\1.37.0;C:\Windows`]),
]

mise_config := {"path": ".mise/config.toml", "contents": {
	"min_version": "2026.10.15",
	"env": {"ALL_LANGS": "dart,python"},
	"tools": {"pipx:cowsay": "latest"},
}}

linux_config := {"path": ".mise/config.linux.toml", "contents": {"bootstrap": {"packages": {
	"apt:bash": "",
	"apt:curl": "",
	"apt:g++": "",
	"dnf:bash": "",
	"dnf:curl": "",
	"dnf:gcc-c++": "",
}}}}

repo(root, nano, extra) := array.concat(
	[
		{"path": "Dockerfile", "contents": root},
		{"path": "Dockerfile.nanoserver", "contents": nano},
		mise_config,
		linux_config,
		{"path": ".mise/config.windows.toml", "contents": {"tools": {"http:busybox": "1.37.0"}}},
		{"path": ".mise/config.python.toml", "contents": {"tools": {"pipx:torch": "latest"}}},
	],
	extra,
)

clean := repo(root_instrs, nano_instrs, [])

with_root_run(line) := repo(array.concat(root_instrs, [instr("run", [line])]), nano_instrs, [])

reports(msgs, fragment) if {
	some msg in msgs
	contains(msg, fragment)
}

test_the_baseline_repo_is_clean if {
	msgs := dockerfile.deny with input as clean
	count(msgs) == 0
}

test_a_tool_named_first_in_an_arg_list_counts_as_disabled if {
	not reports(dockerfile.deny, "pipx:cowsay") with input as clean
}

test_a_pipx_tool_missing_from_the_disable_list_is_denied if {
	nano := array.concat([instr("arg", ["MISE_DT_BASE=cargo:open"])], array.slice(nano_instrs, 1, 4))
	reports(dockerfile.deny, "pipx tool \"pipx:cowsay\" must be in") with input as repo(root_instrs, nano, [])
}

test_a_pipx_tool_in_a_guest_config_nano_does_not_load_is_ignored if {
	not reports(dockerfile.deny, "pipx:torch") with input as clean
}

test_a_pipx_tool_in_a_guest_config_nano_loads_is_denied if {
	dart := {"path": ".mise/config.dart.toml", "contents": {"tools": {"pipx:dartpy": "latest"}}}
	reports(dockerfile.deny, "pipx:dartpy") with input as repo(root_instrs, nano_instrs, [dart])
}

test_a_hard_coded_install_path_at_a_stale_version_is_denied if {
	nano := array.concat(array.slice(nano_instrs, 0, 3), [instr("env", ["PATH", `C:\i\http-busybox\1.36.1;C:\Windows`])])
	reports(dockerfile.deny, "version \"1.36.1\"") with input as repo(root_instrs, nano, [])
}

test_a_heredoc_without_bash_is_denied if {
	reports(dockerfile.deny, "must use bash as interpreter") with input as with_root_run("<<'EOF'")
}

test_a_heredoc_with_an_unquoted_delimiter_is_denied if {
	reports(dockerfile.deny, "must use a quoted delimiter") with input as with_root_run("bash <<EOF")
}

test_a_double_quoted_delimiter_is_accepted if {
	msgs := dockerfile.deny with input as with_root_run(`bash <<"EOF"`)
	count(msgs) == 0
}

test_apt_get_install_without_no_install_recommends_is_denied if {
	reports(dockerfile.deny, "--no-install-recommends") with input as with_root_run("apt-get install -y curl")
}

test_apt_get_install_with_no_install_recommends_is_accepted if {
	msgs := dockerfile.deny with input as with_root_run("apt-get install -y --no-install-recommends curl")
	count(msgs) == 0
}

test_a_commented_apt_line_is_ignored if {
	msgs := dockerfile.deny with input as with_root_run("true\n  # apt install curl")
	count(msgs) == 0
}

test_bare_apt_is_denied if {
	reports(dockerfile.deny, "use `apt-get`, not `apt`") with input as with_root_run("apt update")
}

test_a_missing_package_arg_is_denied if {
	root := array.concat(array.slice(root_instrs, 0, 1), array.slice(root_instrs, 2, 7))
	reports(dockerfile.deny, "ARG APT_PACKAGES not found") with input as repo(root, nano_instrs, [])
}

test_a_package_missing_from_bootstrap_is_denied if {
	root := array.concat([instr("arg", [`COMMON_PACKAGES="bash curl git"`])], array.slice(root_instrs, 1, 7))
	msgs := dockerfile.deny with input as repo(root, nano_instrs, [])
	reports(msgs, "lacks \"apt\" rows for git")
	reports(msgs, "lacks \"dnf\" rows for git")
}

test_a_bootstrap_package_the_args_do_not_install_is_denied if {
	root := array.concat([instr("arg", [`COMMON_PACKAGES="bash"`])], array.slice(root_instrs, 1, 7))
	reports(dockerfile.deny, "has \"apt\" rows for curl") with input as repo(root, nano_instrs, [])
}

test_a_mise_env_drifted_from_all_langs_is_denied if {
	other := {"path": "docker/scenario/Dockerfile", "contents": [instr("env", ["MISE_ENV", "dart"])]}
	reports(dockerfile.deny, "MISE_ENV is \"dart\"") with input as repo(root_instrs, nano_instrs, [other])
}

test_another_image_may_trim_common_packages if {
	other := {"path": "docker/scenario/Dockerfile", "contents": [instr("arg", [`COMMON_PACKAGES="curl"`])]}
	msgs := dockerfile.deny with input as repo(root_instrs, nano_instrs, [other])
	count(msgs) == 0
}

test_a_common_package_absent_from_the_root_is_denied if {
	other := {"path": "docker/scenario/Dockerfile", "contents": [instr("arg", [`COMMON_PACKAGES="curl jq"`])]}
	reports(dockerfile.deny, "names \"jq\"") with input as repo(root_instrs, nano_instrs, [other])
}

test_a_mise_version_behind_min_version_is_denied if {
	other := {"path": "docker/scenario/Dockerfile", "contents": [instr("arg", ["MISE_VERSION=v2026.9.1"])]}
	reports(dockerfile.deny, "MISE_VERSION is \"v2026.9.1\"") with input as repo(root_instrs, nano_instrs, [other])
}
