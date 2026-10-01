//! Covers the pieces `CHECKS.md` is assembled from: tool detection, config paths, Rego summaries and wrapping.
//!
//! The `run` bodies here use made-up tool names, since what is under test is the shell parsing around them.
#![cfg(test)]

use std::collections::BTreeSet;

use et_int_gen::checks::{config_paths, rego_summaries, rule_sources, runner_label, wrap};

/// The set of paths a path-collecting function is expected to return.
fn paths(expected: &[&str]) -> BTreeSet<String> {
    expected.iter().map(|path| (*path).to_owned()).collect()
}

/// Assert the tool `runner_label` names for a `run` body.
fn assert_tool(body: &str, expected: &str) {
    let label = runner_label(body).unwrap();
    assert_eq!(label, expected, "tool for body {body:?}");
}

#[test]
fn single_command_names_its_tool() {
    assert_tool("lintx scan --config config/lintx --error .", "lintx");
    assert_tool("scanz", "scanz");
}

#[test]
fn cargo_is_labelled_by_subcommand_or_package() {
    assert_tool("cargo clippy --keep-going --workspace --tests", "cargo clippy");
    assert_tool("cargo +{{ vars.rust_nightly }} fmt -- --check", "cargo fmt");
    assert_tool("cargo run -q -p et-repo-check", "et-repo-check");
    assert_tool(
        "cargo run --features markdown-help -q -p \"$crate\" -- --markdown-help",
        "cargo run",
    );
}

#[test]
fn plumbing_and_shell_syntax_are_skipped() {
    assert_tool(
        "git ls-files '*Dockerfile' | rg -v x |\n  xargs -0 lintx --config config/lintx.yaml",
        "lintx",
    );
    assert_tool("cfg=config/scanz.toml\nscanz --config \"$cfg\" '**/*.md'", "scanz");
    assert_tool(
        ": \"${TOKEN:=$(gh auth token)}\"\nexport TOKEN\ncargo udeps\n",
        "cargo udeps",
    );
    assert_tool("while read -r line; do\n  lintx \"$line\"\ndone", "lintx");
    assert_tool("# a comment naming fakecmd\nscanz check", "scanz");
}

#[test]
fn substitutions_are_searched_and_quoted_parens_do_not_close_them() {
    assert_tool(
        "rc=0\nout=\"$(cargo audit --config config/audit.toml check 2>&1)\" || rc=$?",
        "cargo audit",
    );
    assert_tool("tidy=\"$(conda_exe some-pkg tidyq)\"\n\"$tidy\" x.c", "tidyq");
    let lock_check = [
        "drift=$(git diff | rg \"^[-+](version|backend) = \" || true)\n",
        "if [ -n \"$drift\" ]; then\n",
        "  echo \"differ; commit the lockfiles\" >&2\n",
        "fi",
    ]
    .concat();
    assert_tool(&lock_check, "Shell-script checks");
}

#[test]
fn env_provided_tools_and_pure_shell_bodies() {
    assert_tool("\"$MVN\" -q compile", "mvn");
    assert_tool("git diff --exit-code -- .dockerignore", "Shell-script checks");
    assert_tool(
        "for cfg in .mise/config*.toml; do\n  echo \"$cfg\"\ndone",
        "Shell-script checks",
    );
}

#[test]
fn config_paths_finds_config_files_and_mise_helpers_only() {
    let found = config_paths(
        "--config config/audit.toml {{ config_root }}/config/scanz.toml -f .mise/fmt-mise.awk .mise/config*.toml",
    )
    .unwrap();
    assert_eq!(
        found,
        paths(&["config/audit.toml", "config/scanz.toml", ".mise/fmt-mise.awk"])
    );
}

#[test]
fn rule_sources_takes_only_flagged_config_paths() {
    let body = [
        "lintx -c config/lintx/sgconfig.yaml\n",
        "scanz --config config/scanz\n",
        "policyq test -p config/policy $files config/inputs/rules\n",
        "schemaz lint --schema \"file:///${PWD#/}/config/schemas/one.schema.json\" Cargo.toml",
    ]
    .concat();
    let expected = [
        "config/lintx/sgconfig.yaml",
        "config/scanz",
        "config/policy",
        "config/schemas/one.schema.json",
    ];
    assert_eq!(rule_sources(&body).unwrap(), paths(&expected));
}

#[test]
fn rego_summary_is_the_last_comment_block_before_each_rule() {
    let source = [
        "# File header.\npackage demo\n\n",
        "# Helper summary.\nis_x(file) if true\n\n",
        "# First rule.\n# More.\ndeny contains msg if {\n\t# body comment\n\tmsg := \"a\"\n}\n\n",
        "# Second rule.\nallowed := {1}\n\ndeny contains msg if {\n\tmsg := \"b\"\n}\n",
    ]
    .concat();
    assert_eq!(
        rego_summaries("demo.rego", &source).unwrap(),
        ["First rule.", "Second rule."]
    );
}

#[test]
fn rego_rule_without_a_summary_comment_is_rejected() {
    let source = "package demo\n\n# Described.\ndeny contains msg if {\n}\n\ndeny contains msg if {\n}\n";
    let error = rego_summaries("demo.rego", source).unwrap_err().to_string();
    assert!(
        error.starts_with("demo.rego:7: rule has no summary comment"),
        "names the file and line: {error}"
    );
}

#[test]
fn wrap_never_starts_a_continuation_line_with_a_block_marker() {
    let text = format!("{} + lock files", "word ".repeat(23));
    let wrapped = wrap("- ", "  ", &text);
    assert!(wrapped.lines().count() > 1, "the text must wrap: {wrapped}");
    assert!(
        wrapped.lines().skip(1).all(|line| !line.trim_start().starts_with("+ ")),
        "no line opens with `+`: {wrapped}"
    );
}

#[test]
fn wrap_fills_lines_to_the_width_with_a_continuation_prefix() {
    let text = "word ".repeat(40);
    let wrapped = wrap("- ", "  ", &text);
    let lines: Vec<&str> = wrapped.lines().collect();
    assert!(lines.len() > 1, "40 words must not fit on one line");
    assert!(
        lines.iter().all(|line| line.len() <= 120),
        "every line within the width: {wrapped}"
    );
    assert!(
        lines.first().is_some_and(|line| line.starts_with("- word")),
        "first prefix applied: {wrapped}"
    );
    assert!(
        lines.iter().skip(1).all(|line| line.starts_with("  word")),
        "continuation prefix applied: {wrapped}"
    );
}
