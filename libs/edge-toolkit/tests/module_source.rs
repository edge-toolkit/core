//! Covers how a resolved or missing mise-installed module is added to the modules service's search paths.
#![cfg(test)]

use std::path::PathBuf;

use edge_toolkit::config::{npm_module, push_module};
use et_test_otlp::events::capture_events;
use tracing::Level;

#[test]
fn a_found_module_is_added_and_a_missing_one_warns_that_its_route_will_404() {
    let mut paths = Vec::new();
    let events = capture_events(|| {
        push_module(
            &mut paths,
            &npm_module("stats-gl"),
            Some(PathBuf::from("/opt/stats-gl")),
        );
        push_module(&mut paths, &npm_module("@scope/pkg"), None);
    });

    assert_eq!(paths, [PathBuf::from("/opt/stats-gl")]);
    let missing = concat!(
        "npm:@scope/pkg install path not found via `mise where` -- requests to /modules/@scope/pkg/* will 404. ",
        "Run `mise install npm:@scope/pkg` and verify the install."
    );
    assert_eq!(
        events,
        [
            (
                Level::INFO,
                "Resolved npm:stats-gl modules path: /opt/stats-gl".to_owned()
            ),
            (Level::WARN, missing.to_owned()),
        ]
    );
}
