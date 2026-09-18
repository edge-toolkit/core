# Unit tests for the regex-driven halves of the mise policy, run by `conftest verify`.
#
# Two things here cannot be reached by running the real task. The `is_mise` path predicate normalises the
# backslash separators conftest reports only when it is handed the `.mise` DIRECTORY, which no Linux or macOS
# run produces; that normalisation gates every rule in the package, so losing it empties the whole policy on one
# lane alone. And the regex rules -- compgen, an embedded version, the install-path drift scan -- all fail open,
# so a broken pattern reads exactly like a clean config in a run over files that already comply.
package mise_test

import data.mise

config(path, contents) := [{"path": path, "contents": contents}]

task(name, body) := {"tasks": {name: body}}

reports(msgs, fragment) if {
	some msg in msgs
	contains(msg, fragment)
}

# conftest reports native separators when walking a directory, so a Windows lane sees `.mise\config.toml`.
# Every rule in the package hangs off this predicate, so the normalisation is the policy's on/off switch there.
test_windows_separators_still_match_the_mise_config if {
	msgs := mise.deny with input as config(".mise\\config.toml", task("probe", {"run": "compgen -c"}))
	reports(msgs, "compgen")
}

test_posix_separators_match_the_mise_config if {
	msgs := mise.deny with input as config(".mise/config.toml", task("probe", {"run": "compgen -c"}))
	reports(msgs, "compgen")
}

test_a_path_outside_mise_is_ignored if {
	msgs := mise.deny with input as config("config/other.toml", task("probe", {"run": "compgen -c"}))
	count(msgs) == 0
}

# `compgen` is matched on word boundaries, so a longer word that merely contains it is not the bash builtin.
test_compgen_inside_a_longer_word_is_not_the_builtin if {
	msgs := mise.deny with input as config(".mise/config.toml", task("probe", {"run": "echo recompgenerate"}))
	count(msgs) == 0
}

# The maintainer-only config never runs on Nano Server, so it is exempt by path.
test_maint_config_may_use_compgen if {
	msgs := mise.deny with input as config(".mise/config.maint.toml", task("probe", {"run": "compgen -c"}))
	count(msgs) == 0
}

# A version baked into a run body goes stale silently, because the drift check only reads [vars]/[env].
test_version_literal_in_a_run_body_is_flagged if {
	msgs := mise.deny with input as config(".mise/config.toml", task("fetch", {"run": "curl -O host/v1.2.3/x"}))
	reports(msgs, "embeds version-like text")
}

test_two_part_number_in_a_run_body_is_not_a_version if {
	msgs := mise.deny with input as config(".mise/config.toml", task("fetch", {"run": "curl -O host/v1.2/x"}))
	count(msgs) == 0
}

test_maint_config_may_name_versions_in_a_run_body if {
	msgs := mise.deny with input as config(".mise/config.maint.toml", task("pub", {"run": "gh release view v1.2.3"}))
	count(msgs) == 0
}

# python is the one tool that has to carry a patch, because mise only symlinks the X.Y alias on some platforms.
test_python_pinned_to_a_minor_is_flagged if {
	msgs := mise.deny with input as config(".mise/config.toml", {"tools": {"python": "3.13"}})
	reports(msgs, "full version triple")
}

test_python_pinned_to_a_triple_is_accepted if {
	msgs := mise.deny with input as config(".mise/config.toml", {"tools": {"python": "3.13.1"}})
	count(msgs) == 0
}

test_python_triple_is_read_out_of_the_table_form_too if {
	msgs := mise.deny with input as config(".mise/config.toml", {"tools": {"python": {"version": "3.13.1"}}})
	count(msgs) == 0
}

# An install path names a tool's version as a path segment, which has to track the [tools] pin.
test_install_path_matching_the_pin_is_clean if {
	contents := {"tools": {"ripgrep": "14.1.1"}, "vars": {"rg": "installs/ripgrep/14.1.1/bin"}}
	msgs := mise.deny with input as config(".mise/config.toml", contents)
	count(msgs) == 0
}

test_install_path_behind_the_pin_is_flagged if {
	contents := {"tools": {"ripgrep": "14.1.1"}, "vars": {"rg": "installs/ripgrep/14.1.0/bin"}}
	msgs := mise.deny with input as config(".mise/config.toml", contents)
	reports(msgs, "keep them in sync")
}

# mise flattens `:` and `/` into `-` to name an install dir, and the github backend flattens `_` as well.
test_underscore_flattened_install_dir_is_recognised if {
	drifted := "installs/github-brechtsanders-winlibs-mingw/12.0/bin"
	contents := {"tools": {"github:brechtsanders/winlibs_mingw": "13.0"}, "vars": {"cc": drifted}}
	msgs := mise.deny with input as config(".mise/config.toml", contents)
	reports(msgs, "keep them in sync")
}

# A multiline run needs the strict shell, or a failing command in the middle is masked.
test_multiline_run_without_the_strict_shell_is_flagged if {
	msgs := mise.deny with input as config(".mise/config.toml", task("build", {"run": "cd x\nmake"}))
	reports(msgs, "multiline run")
}

test_multiline_run_with_the_strict_shell_is_accepted if {
	body := {"run": "cd x\nmake", "shell": "{{ vars.task_shell }}"}
	msgs := mise.deny with input as config(".mise/config.toml", task("build", body))
	count(msgs) == 0
}

# A declared arg the body never reads is the parfit-fmt failure mode: accepted, then silently ignored.
test_declared_arg_must_be_read_by_the_body if {
	body := {"usage": `arg "[file]..." var=#true`, "run": "git ls-files '*.rs' | xargs -r parfit"}
	msgs := mise.deny with input as config(".mise/config.toml", task("parfit-fmt", body))
	reports(msgs, "declares a usage arg its run never reads as $usage_file")
}

test_declared_arg_that_is_read_is_accepted if {
	body := {"usage": `arg "[file]..." var=#true`, "run": `echo "$usage_file" | xargs -r parfit`}
	msgs := mise.deny with input as config(".mise/config.toml", task("parfit-fmt", body))
	count(msgs) == 0
}

# A required arg is declared `<name>` rather than `[name]`, and reaches the body under the same rule.
test_required_arg_is_checked_too if {
	body := {"usage": `arg "<package>" help="Workspace package"`, "run": "cargo clippy -p \"$USAGE_PACKAGE\""}
	msgs := mise.deny with input as config(".mise/config.toml", task("clippy-pkg", body))
	reports(msgs, "declares a usage arg its run never reads as $usage_package")
}

# A flag's dashes become underscores, so `--dry-run` has to be read as `$usage_dry_run`.
test_flag_dashes_become_underscores if {
	body := {"usage": `flag "--dry-run" help="Print the plan"`, "run": "echo \"${usage_dry_run:-}\""}
	msgs := mise.deny with input as config(".mise/config.toml", task("release", body))
	count(msgs) == 0
}

test_flag_that_is_never_read_is_denied if {
	body := {"usage": `flag "--execute" help="Actually publish"`, "run": "cargo release"}
	msgs := mise.deny with input as config(".mise/config.toml", task("publish", body))
	reports(msgs, "declares a usage arg its run never reads as $usage_execute")
}

# The uppercase spelling is reported on its own terms, so the message names the mistake rather than the symptom.
test_uppercase_usage_variable_is_denied if {
	body := {"run": "out=\"${USAGE_OUT:?output directory required}\""}
	msgs := mise.deny with input as config(".mise/config.toml", task("oci", body))
	reports(msgs, "reads $USAGE_* -- mise exports usage args lowercased")
}

test_lowercase_usage_variable_is_not_denied if {
	body := {"run": "out=\"${usage_out:?output directory required}\""}
	msgs := mise.deny with input as config(".mise/config.toml", task("oci", body))
	count(msgs) == 0
}

# A task with no usage spec at all is none of this rule's business.
test_a_task_without_a_usage_spec_is_ignored if {
	msgs := mise.deny with input as config(".mise/config.toml", task("plain", {"run": "cargo build"}))
	count(msgs) == 0
}
