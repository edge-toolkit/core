# Cross-checks for GitHub Actions workflows against the .mise/config*.toml pins, run with `--namespace gha_mise`.
# Evaluated over the workflow YAML plus the .mise/config*.toml files combined (--combine, auto-detected parsers).
# A workflow that hard-codes a toolchain version via an action input must match the corresponding mise [tools] pin,
# so the CI runner SDK and the mise-built modules compile against the same toolchain.
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
# Every string is scanned rather than only those naming an `installs` dir: the Windows install root moved to
# a short MISE_INSTALLS_DIR that carries no such segment, and an `installs` prefilter would have taken the
# busybox shell path out of the matcher's reach without a word. version_drift keys on `<tool-dir>/<version>`,
# which is selective enough by itself.
deny contains msg if {
	some file in input
	endswith(file.path, ".yaml")
	walk(file.contents, [_, value])
	is_string(value)
	some d in mise.version_drift(value)
	msg := sprintf(
		"%s: a workflow string embeds %q version %q, but [tools] pins it to %q -- keep them in sync",
		[file.path, d.dir, d.seg, d.pinned],
	)
}

# Every workflow's MISE_ENV must be the list .mise/config.toml declares as ALL_LANGS.
# gha.rego pins the same value per-workflow, but against a literal in the policy itself, so it cannot tell whether
# that literal still matches the repo. Anchoring here instead makes ALL_LANGS the thing everything else tracks:
# adding a config.<lang>.toml and updating ALL_LANGS lights up every workflow that was left behind.
all_langs := value if {
	some file in input
	mise.is_mise(file)
	endswith(replace(file.path, "\\", "/"), ".mise/config.toml")
	value := file.contents.env.ALL_LANGS
}

deny contains msg if {
	some file in input
	endswith(file.path, ".yaml")
	declared := file.contents.env.MISE_ENV
	declared != all_langs
	msg := sprintf(
		"%s: env.MISE_ENV is %q but .mise/config.toml's ALL_LANGS is %q -- keep the guest-language copies in sync",
		[file.path, declared, all_langs],
	)
}
