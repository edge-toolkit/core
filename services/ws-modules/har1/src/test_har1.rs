use super::*;

#[test]
fn softmax_distribution_preserves_order_and_normalizes() {
    let logits = vec![2.0, 1.0, 0.1];
    let probs = softmax(&logits);

    assert_eq!(probs.len(), 3);
    let sum: f64 = probs.iter().sum();
    assert!((sum - 1.0).abs() < 1e-6);
    assert!(probs[0] > probs[1]);
    assert!(probs[1] > probs[2]);
}

#[test]
fn softmax_handles_empty_equal_and_large_values() {
    assert!(softmax(&[]).is_empty());

    let equal = softmax(&[7.0, 7.0, 7.0]);
    assert!(equal.iter().all(|value| (*value - (1.0 / 3.0)).abs() < 1e-6));

    let large = softmax(&[1000.0, 1001.0, 999.0]);
    assert_eq!(large.len(), 3);
    assert!(large.iter().all(|value| value.is_finite()));
    assert!((large.iter().sum::<f64>() - 1.0).abs() < 1e-6);
    assert!(large[1] > large[0]);
    assert!(large[0] > large[2]);
}

#[test]
fn gravity_and_rotation_conversions_handle_positive_negative_and_zero() {
    assert_eq!(to_g(0.0), 0.0);
    assert!((to_g(9.80665) - 1.0).abs() < 1e-6);
    assert!((to_g(-9.80665) + 1.0).abs() < 1e-6);

    assert_eq!(degrees_to_radians(0.0), 0.0);
    assert!((degrees_to_radians(180.0) - std::f64::consts::PI).abs() < 1e-6);
    assert!((degrees_to_radians(-90.0) + std::f64::consts::FRAC_PI_2).abs() < 1e-6);
}

#[test]
fn flatten_samples_preserves_sample_order_and_feature_order() {
    let mut samples = VecDeque::new();
    let mut first = [0.0; HAR_FEATURE_COUNT];
    let mut second = [0.0; HAR_FEATURE_COUNT];
    for index in 0..HAR_FEATURE_COUNT {
        first[index] = index as f32;
        second[index] = (10 + index) as f32;
    }
    samples.push_back(first);
    samples.push_back(second);

    let flattened = flatten_samples(&samples);

    assert_eq!(flattened.len(), 2 * HAR_FEATURE_COUNT);
    assert_eq!(&flattened[..HAR_FEATURE_COUNT], &first);
    assert_eq!(&flattened[HAR_FEATURE_COUNT..], &second);
}

#[test]
fn flatten_samples_handles_empty_buffer() {
    let samples = VecDeque::new();

    assert!(flatten_samples(&samples).is_empty());
}

#[test]
fn format_number_rejects_non_finite_values() {
    assert_eq!(format_number(12.3456, 2), "12.35");
    assert_eq!(format_number(f64::NAN, 2), "n/a");
    assert_eq!(format_number(f64::INFINITY, 2), "n/a");
}
