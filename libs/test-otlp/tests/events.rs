//! Covers `capture_events`: what it records, in what order, and that it stops at the closure's end.
#![cfg(test)]

use et_test_otlp::events::capture_events;
use tracing::{Level, info, warn};

#[test]
fn records_each_event_level_and_message_in_order() {
    let captured = capture_events(|| {
        info!(count = 2_u32, "first {}", "event");
        warn!("second");
    });
    assert_eq!(
        captured,
        [
            (Level::INFO, "first event".to_owned()),
            (Level::WARN, "second".to_owned())
        ]
    );
}

#[test]
fn an_event_with_no_message_records_an_empty_one() {
    let captured = capture_events(|| info!(count = 1_u32));
    assert_eq!(captured, [(Level::INFO, String::default())]);
}

#[test]
fn nothing_is_recorded_outside_the_closure() {
    info!("before");
    let captured = capture_events(|| {});
    assert_eq!(captured, []);
}
