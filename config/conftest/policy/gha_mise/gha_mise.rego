# Cross-checks for GitHub Actions workflows against the .mise/config*.toml pins, run with `--namespace gha_mise`.
# Evaluated over the workflow YAML plus the .mise/config*.toml files combined (--combine, auto-detected parsers).
# A workflow that hard-codes a toolchain version via an action input (test-msvc.yaml installs the .NET SDK with
# actions/setup-dotnet's `dotnet-version`, deliberately bypassing mise) must match the matching mise [tools] pin, so
# the CI runner SDK and the mise-built modules compile against the same toolchain.
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
