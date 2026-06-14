# Dockerfile.nanoserver hard-codes several mise install-dir paths (LLVMBIN, the
# busybox shell, the python dir on PATH), each embedding a tool's pinned version
# as a path segment. mise can't inject those paths into the Dockerfile, so they
# are manual copies of the [tools] pins in .mise/config*.toml. Evaluated over the
# Dockerfile plus those TOMLs combined (--combine, auto-detected parsers), this
# fails if any embedded version drifts from its pin, reusing the matcher in
# mise.rego. Run with `--namespace dockerfile`.
package dockerfile

import data.mise

# Every string argument of an ENV/RUN instruction in the Dockerfile (its parsed
# contents is the array of instruction objects; the TOMLs parse to objects).
docker_strings contains entry if {
	some file in input
	is_array(file.contents)
	some instr in file.contents
	instr.Cmd in {"env", "run"}
	some value in instr.Value
	is_string(value)
	entry := {"path": file.path, "value": value}
}

deny contains msg if {
	some entry in docker_strings
	contains(entry.value, "installs")
	some d in mise.version_drift(entry.value)
	msg := sprintf(
		"%s: hard-codes %q version %q, but [tools] pins it to %q -- keep them in sync",
		[entry.path, d.dir, d.seg, d.pinned],
	)
}
