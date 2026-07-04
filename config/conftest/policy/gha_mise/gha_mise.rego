# Cross-checks for GitHub Actions workflows against the .mise/config*.toml pins, run with `--namespace gha_mise`.
# Evaluated over the workflow YAML plus the .mise/config*.toml files combined (--combine, auto-detected parsers).
# A workflow that hard-codes a toolchain version via an action input must match the corresponding mise [tools] pin,
# so the CI runner SDK and the mise-built modules compile against the same toolchain. No workflow currently does
# (the old test-msvc.yaml installed the .NET SDK via actions/setup-dotnet before the msvc lane moved onto mise);
# the rule stays as the guard for any reappearance.
package gha_mise

import data.mise

# The mise-managed .NET SDK pin (.mise/config.dotnet.toml: dotnet = "<ver>").
dotnet_pin := version if {
	some file in input
	mise.is_mise(file)
	version := file.contents.tools.dotnet
}

# Every setup-dotnet `dotnet-version` must equal the mise `dotnet` pin.
deny contains msg if {
	some file in input
	endswith(file.path, ".yaml")
	some job in file.contents.jobs
	some step in job.steps
	declared := step.with["dotnet-version"]
	declared != dotnet_pin
	msg := sprintf(
		"%s: setup-dotnet dotnet-version %q must match the mise dotnet pin %q in .mise/config.dotnet.toml",
		[file.path, declared, dotnet_pin],
	)
}

# Any workflow string embedding a mise install path must track the matching [tools] pin.
# Same version-drift check the .mise [vars]/[env] and Dockerfile passes run (data.mise.version_drift);
# here it covers e.g. the Windows jobs' default `shell:`, which hard-codes the http-busybox install path
# that a busybox bump would otherwise silently strand. walk() visits every nested string in the workflow.
deny contains msg if {
	some file in input
	endswith(file.path, ".yaml")
	walk(file.contents, [_, value])
	is_string(value)
	contains(value, "installs")
	some d in mise.version_drift(value)
	msg := sprintf(
		"%s: a workflow string embeds %q version %q, but [tools] pins it to %q -- keep them in sync",
		[file.path, d.dir, d.seg, d.pinned],
	)
}
