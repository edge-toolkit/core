use super::*;

#[test]
fn clamp_bounds_values() {
    assert_eq!(clamp(5.0, 0.0, 10.0), 5.0);
    assert_eq!(clamp(0.0, 0.0, 10.0), 0.0);
    assert_eq!(clamp(10.0, 0.0, 10.0), 10.0);
    assert_eq!(clamp(-1.0, 0.0, 10.0), 0.0);
    assert_eq!(clamp(11.0, 0.0, 10.0), 10.0);
}

fn detection(score: f64, box_coords: [f64; 4]) -> Detection {
    Detection {
        label: "face".into(),
        class_index: 0,
        score,
        box_coords,
    }
}

#[test]
fn iou_uses_inclusive_pixel_coordinates() {
    let left = detection(1.0, [0.0, 0.0, 10.0, 10.0]);
    let right = detection(1.0, [5.0, 5.0, 15.0, 15.0]);

    let iou = compute_iou(&left, &right);

    assert!((iou - (36.0 / 206.0)).abs() < 1e-6);
}

#[test]
fn iou_handles_identical_and_non_overlapping_boxes() {
    let left = detection(1.0, [0.0, 0.0, 10.0, 10.0]);
    let identical = detection(1.0, [0.0, 0.0, 10.0, 10.0]);
    let separate = detection(1.0, [20.0, 20.0, 30.0, 30.0]);
    let corner_touching = detection(1.0, [10.0, 10.0, 20.0, 20.0]);

    assert!((compute_iou(&left, &identical) - 1.0).abs() < 1e-6);
    assert_eq!(compute_iou(&left, &separate), 0.0);
    assert!((compute_iou(&left, &corner_touching) - (1.0 / 241.0)).abs() < 1e-6);
}

#[test]
fn nms_keeps_highest_scored_overlapping_box_and_distant_boxes() {
    let detections = vec![
        detection(0.7, [50.0, 50.0, 60.0, 60.0]),
        detection(0.8, [1.0, 1.0, 11.0, 11.0]),
        detection(0.9, [0.0, 0.0, 10.0, 10.0]),
    ];

    let filtered = apply_nms(detections, 0.5);

    assert_eq!(filtered.len(), 2);
    assert_eq!(filtered[0].score, 0.9);
    assert_eq!(filtered[1].score, 0.7);
}

#[test]
fn nms_keeps_boxes_when_iou_equals_threshold() {
    let filtered = apply_nms(
        vec![
            detection(0.9, [0.0, 0.0, 10.0, 10.0]),
            detection(0.8, [0.0, 0.0, 10.0, 10.0]),
        ],
        1.0,
    );

    assert_eq!(filtered.len(), 2);
}

#[test]
fn softmax_handles_empty_equal_and_large_values() {
    assert!(softmax(&[]).is_empty());

    let equal = softmax(&[4.0, 4.0, 4.0, 4.0]);
    assert!(equal.iter().all(|value| (*value - 0.25).abs() < 1e-6));

    let large = softmax(&[1000.0, 1001.0]);
    assert_eq!(large.len(), 2);
    assert!(large.iter().all(|value| value.is_finite()));
    assert!((large.iter().sum::<f64>() - 1.0).abs() < 1e-6);
    assert!(large[1] > large[0]);
}

#[test]
fn retinaface_prior_count_matches_model_input_shape() {
    let priors = build_retinaface_priors(FACE_INPUT_HEIGHT_F64, FACE_INPUT_WIDTH_F64);

    assert_eq!(priors.len(), 15_960);
    assert!((priors[0][0] - (4.0 / FACE_INPUT_WIDTH_F64)).abs() < 1e-6);
    assert!((priors[0][1] - (4.0 / FACE_INPUT_HEIGHT_F64)).abs() < 1e-6);
    assert!((priors[0][2] - (16.0 / FACE_INPUT_WIDTH_F64)).abs() < 1e-6);
    assert!((priors[0][3] - (16.0 / FACE_INPUT_HEIGHT_F64)).abs() < 1e-6);
}

#[test]
fn retinaface_zero_offsets_decode_to_prior_box() {
    let decoded = decode_retinaface_box([0.0, 0.0, 0.0, 0.0], [0.5, 0.5, 0.25, 0.5]);

    assert!((decoded[0] - 0.375).abs() < 1e-6);
    assert!((decoded[1] - 0.25).abs() < 1e-6);
    assert!((decoded[2] - 0.625).abs() < 1e-6);
    assert!((decoded[3] - 0.75).abs() < 1e-6);
}
