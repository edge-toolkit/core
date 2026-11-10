# Unit tests for the workflow-versus-mise-pin cross-checks, run by `conftest verify`.
#
# The real task only ever sees workflows that agree with the pins, which looks the same as a rule that never matches.
package gha_mise_test

import data.gha_mise

configs := [
	{"path": ".mise/config.toml", "contents": {"env": {"ALL_LANGS": "dart,python"}, "tools": {}}},
	{"path": ".mise/config.dotnet.toml", "contents": {"tools": {"dotnet": "10.0.100"}}},
	{"path": ".mise/config.windows.toml", "contents": {"tools": {"http:busybox": "1.37.0"}}},
]

with_workflow(contents) := array.concat(configs, [{"path": ".github/workflows/probe.yaml", "contents": contents}])

dotnet_step(version) := {"jobs": {"build": {"steps": [{"with": {"dotnet-version": version}}]}}}

shell_job(shell) := {"jobs": {"build": {"defaults": {"run": {"shell": shell}}, "steps": []}}}

reports(msgs, fragment) if {
	some msg in msgs
	contains(msg, fragment)
}

test_a_dotnet_version_matching_the_pin_is_accepted if {
	not reports(gha_mise.deny, "setup-dotnet") with input as with_workflow(dotnet_step("10.0.100"))
}

test_a_drifted_dotnet_version_is_denied if {
	reports(gha_mise.deny, "dotnet-version \"9.0.300\"") with input as with_workflow(dotnet_step("9.0.300"))
}

test_an_install_path_at_the_pinned_version_is_accepted if {
	shell := `C:\i\http-busybox\1.37.0\busybox.exe sh -e {0}`
	not reports(gha_mise.deny, "embeds") with input as with_workflow(shell_job(shell))
}

test_an_install_path_at_a_stale_version_is_denied if {
	shell := `C:\i\http-busybox\1.36.1\busybox.exe sh -e {0}`
	reports(gha_mise.deny, "version \"1.36.1\"") with input as with_workflow(shell_job(shell))
}

test_mise_env_matching_all_langs_is_accepted if {
	not reports(gha_mise.deny, "env.MISE_ENV") with input as with_workflow({"env": {"MISE_ENV": "dart,python"}})
}

test_mise_env_drifted_from_all_langs_is_denied if {
	reports(gha_mise.deny, "env.MISE_ENV is \"dart\"") with input as with_workflow({"env": {"MISE_ENV": "dart"}})
}
