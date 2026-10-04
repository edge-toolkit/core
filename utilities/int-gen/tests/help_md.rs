//! Covers `help-md`: which crates it detects, every committed HELP.md, and the environment-section renderer.
#![cfg(test)]

use edge_toolkit::config::get_project_root;
use et_int_gen::first_difference;
use et_int_gen::help::{binary_crates, clap_tree, render_env_section};
use serde_json::json;

#[test]
fn clap_trees_carry_no_html_and_no_skipped_heading_levels() {
    let command = clap::Command::new("demo").arg(clap::Arg::new("verbose").long("verbose").help("Talk more"));
    let markdown = clap_tree(&command);
    assert!(markdown.contains("\n## `demo`\n"));
    assert!(markdown.contains("\n### Options\n"));
    assert!(!markdown.contains("######"));
    assert!(!markdown.contains("<hr/>"));
    assert!(!markdown.contains("<small>"));
}

#[test]
fn committed_help_files_match_their_command_trees_and_env_configs() {
    et_int_gen::check_help().unwrap();
}

#[test]
fn a_stale_file_names_its_first_differing_line() {
    assert_eq!(
        first_difference("a\nb\nc\n", "a\nB\nc\n"),
        "line 2 is `b` but renders as `B`"
    );
    assert_eq!(
        first_difference("a\n", "a\nb\n"),
        "line 2 is end of file but renders as `b`"
    );
    assert_eq!(
        first_difference("a\n", "a"),
        "the lines match, so only a line ending or the final newline differs"
    );
}

#[test]
fn detects_every_binary_using_clap_or_serde_env() {
    let crates = binary_crates(&get_project_root()).unwrap();
    let documented: Vec<(&str, bool, bool)> = crates
        .iter()
        .filter(|krate| krate.uses_clap || krate.reads_env)
        .map(|krate| (krate.name.as_str(), krate.uses_clap, krate.reads_env))
        .collect();
    assert_eq!(
        documented,
        [
            ("et-cli", true, false),
            ("et-int-gen", true, false),
            ("et-onnx", true, false),
            ("et-ws-pyo3-runner", false, true),
            ("et-ws-server", true, true),
            ("et-ws-wasi-runner", false, true),
            ("et-ws-web-runner", false, true),
        ]
    );
}

#[test]
fn renders_each_leaf_under_its_serde_env_name() {
    let long = ["word"; 30].join(" ");
    let schema = json!({
        "type": "object",
        "properties": {
            "verbose": {"description": long, "type": ["boolean", "null"]},
            "runner": {"description": "Runner.", "$ref": "#/$defs/Runner"},
            "paths": {
                "description": "Dirs, see [`default_dirs`] and [`docs`](https://example.com).",
                "type": "array",
                "items": {"type": "string"},
                "default": ["a", "b"]
            },
            "otlp": {"anyOf": [{"$ref": "#/$defs/Otlp"}, {"type": "null"}]}
        },
        "required": ["runner"],
        "$defs": {
            "Otlp": {"type": "object", "required": ["url"], "properties": {
                "url": {"type": "string"}, "label": {"type": "string", "default": null},
                "protocol": {"$ref": "#/$defs/Protocol", "default": "Binary"}
            }},
            "Protocol": {"description": "Wire protocol.", "oneOf": [{"const": "Binary"}, {"const": "JSON"}]},
            "Runner": {"type": "object", "required": ["module"], "properties": {
                "retries": {"type": "integer", "default": 3_i32},
                "module": {"description": "Module to run.", "type": "string"}
            }}
        }
    });
    let first_line = ["word"; 24].join(" ");
    let second_line = ["word"; 6].join(" ");
    let expected = [
        "## Environment variables\n\n",
        "### `OTLP_LABEL`\n\nType: string.\n\n",
        "### `OTLP_PROTOCOL`\n\nType: one of `Binary`, `JSON`. Default: `Binary`.\n\nWire protocol.\n\n",
        "### `OTLP_URL`\n\nType: string.\n\n",
        "### `PATHS`\n\nType: comma-separated list of string. Default: `a,b`.\n\n",
        "Dirs, see `default_dirs` and [`docs`](https://example.com).\n\n",
        "### `RUNNER_MODULE`\n\nType: string. Required.\n\nModule to run.\n\n",
        "### `RUNNER_RETRIES`\n\nType: integer. Default: `3`.\n\n",
        &format!("### `VERBOSE`\n\nType: boolean.\n\n{first_line}\n{second_line}\n\n"),
    ]
    .concat();
    assert_eq!(render_env_section(&schema).unwrap(), expected);
}
