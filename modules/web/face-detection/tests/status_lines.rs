//! Host-side check of the pure status-panel line builder (the wasm interop stays untested here).

#![cfg(test)]

use et_ws_face_detection::face_status_lines;

#[test]
fn includes_a_separated_best_box_section_when_a_detection_exists() {
    let lines = face_status_lines(
        "input",
        &[String::from("out0"), String::from("out1")],
        2,
        0.9876_f64,
        "2026-08-17T00:00:00Z",
        Some([1.0_f64, 2.0_f64, 3.0_f64, 4.0_f64]),
    );
    assert!(
        lines.contains(&String::default()),
        "missing the blank separator: {lines:?}"
    );
    assert_eq!(lines.last().unwrap(), "best box: 1.0, 2.0, 3.0, 4.0");
    assert!(
        lines.contains(&String::from("detections: 2")),
        "missing the count: {lines:?}"
    );
}

#[test]
fn omits_the_best_box_section_without_detections() {
    let lines = face_status_lines("input", &[], 0, 0.0_f64, "2026-08-17T00:00:00Z", None);
    assert!(!lines.contains(&String::default()), "unexpected separator: {lines:?}");
}
