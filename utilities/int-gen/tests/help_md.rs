//! Covers `help-md`: every committed HELP.md matches what the generator renders from its crate's clap tree.
#![cfg(test)]

use edge_toolkit::config::get_project_root;

#[test]
fn committed_help_files_match_the_command_trees() {
    et_int_gen::check_help().unwrap();
}

#[test]
fn every_utility_has_a_rendered_help_file() {
    let rendered = et_int_gen::help::render(&get_project_root()).unwrap();
    let paths: Vec<String> = rendered.iter().map(|(path, _)| path.display().to_string()).collect();
    assert_eq!(
        paths,
        [
            "utilities/cli/HELP.md",
            "utilities/int-gen/HELP.md",
            "utilities/onnx/HELP.md"
        ]
    );
}
