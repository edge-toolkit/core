//! The Python twin under `libs/python-org/` writes the organisation out a second time; this keeps the two equal.
//!
//! It lives here rather than in et-org because it needs the project root, and an et-org dev-dependency on
//! edge-toolkit would be a cycle: each would need the other's new version on crates.io before it could publish.
#![cfg(test)]

use edge_toolkit::config::get_project_root;

#[test]
fn the_python_twin_writes_the_same_organisation() {
    let twin = get_project_root().join("libs/python-org/et_org/__init__.py");
    let source = std::fs::read_to_string(twin).unwrap();
    let assignments: Vec<&str> = source.lines().filter(|line| line.starts_with("ORG = ")).collect();
    assert_eq!(assignments, [format!("ORG = \"{}\"", et_org::ORG)]);
}
