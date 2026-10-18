# Every `--from=` in a published scenario Dockerfile names a stage the file declared before it.
# Run with `--namespace dockerfile_stages` over each `verification/published/**/Dockerfile`, one file per input, as
# the dockerfile parser hands it over: an array of instructions, each carrying its `Stage` index.
#
# `COPY --from=` and `RUN --mount=...,from=` resolve a name that matches no stage as an image reference and pull it,
# so even one of the stage names a published Dockerfile may use -- `released`, `deps` -- brings in an image nobody
# declared if the file never builds that stage. Only a stage declared earlier counts: a later one is not yet built
# where it is used.
package dockerfile_stages

# The `AS` name of each `FROM`, with the stage it opens.
declared contains {"name": lower(instr.Value[2]), "stage": instr.Stage} if {
	some instr in input
	instr.Cmd == "from"
	count(instr.Value) == 3
	lower(instr.Value[1]) == "as"
}

# The source names one instruction flag points `from` at: `--from=<name>`, or `from=<name>` inside a `--mount=`.
flag_sources(flag) := {lower(trim_prefix(flag, "--from="))} if {
	startswith(flag, "--from=")
}

# The `from=` options of a `--mount=` flag, which a `RUN` takes a source from just as `COPY --from=` does.
flag_sources(flag) := {lower(trim_prefix(option, "from=")) |
	some option in split(trim_prefix(flag, "--mount="), ",")
	startswith(option, "from=")
} if {
	startswith(flag, "--mount=")
}

# Every stage name an instruction's flags take a source from, with the stage the instruction is in.
referenced contains {"name": name, "stage": instr.Stage} if {
	some instr in input
	some flag in instr.Flags
	some name in flag_sources(flag)
}

# A `--from=` naming no stage declared before it would pull an image of that name.
deny contains msg if {
	some ref in referenced
	not declared_before(ref)
	msg := sprintf(
		"--from=%s names no stage declared before it, so the build would pull an image called %q instead",
		[ref.name, ref.name],
	)
}

# Whether a stage of the referenced name opens before the stage the reference is in.
declared_before(ref) if {
	some stage in declared
	stage.name == ref.name
	stage.stage < ref.stage
}
