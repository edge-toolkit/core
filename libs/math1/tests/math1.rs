//! Covers the kernel against the canonical model, each way a frame is or is not the input pointer, and the output.
#![cfg(test)]

use et_math1::{
    INPUT_FILENAME, INPUT_TYPE, Math1Input, PointerError, fed_avg, output_json, parse_pointer, pointer_frame,
};
use et_ws_test_server::math1::{MATH1_INPUT_JSON, verify_math1_model};
use serde_json::Value;

#[test]
fn fed_avg_computes_the_canonical_model() {
    let input: Math1Input = serde_json::from_str(MATH1_INPUT_JSON).unwrap();
    let model = fed_avg(&input);
    verify_math1_model(model.weight, model.bias).unwrap();
}

#[test]
fn the_pointer_frame_parses_back_to_the_input_it_names() {
    let frame = pointer_frame("bucket-1");
    let value: Value = serde_json::from_str(&frame).unwrap();
    assert_eq!(value["type"], INPUT_TYPE);
    let pointer = parse_pointer(&frame).unwrap();
    assert_eq!(pointer.bucket, "bucket-1");
    assert_eq!(pointer.filename, INPUT_FILENAME);
}

#[test]
fn a_frame_that_is_not_json_is_not_a_pointer() {
    let error = parse_pointer("not json").unwrap_err();
    let expected = serde_json::from_str::<Value>("not json").unwrap_err().to_string();
    assert!(matches!(error, PointerError::Json(_)));
    assert_eq!(error.to_string(), expected);
}

#[test]
fn another_message_is_not_a_pointer() {
    let error = parse_pointer(r#"{"type":"et-connect-ack"}"#).unwrap_err();
    assert!(matches!(error, PointerError::OtherType));
    assert_eq!(error.to_string(), "not a math1-input frame");
}

#[test]
fn a_pointer_naming_no_file_is_malformed() {
    let error = parse_pointer(r#"{"type":"math1-input","bucket":"bucket-1"}"#).unwrap_err();
    assert!(matches!(error, PointerError::Json(_)));
    assert!(error.to_string().contains("filename"));
}

#[test]
fn the_output_names_the_module_and_carries_the_model() {
    let input: Math1Input = serde_json::from_str(MATH1_INPUT_JSON).unwrap();
    let model = fed_avg(&input);
    let output: Value = serde_json::from_str(&output_json("twin", model)).unwrap();
    assert_eq!(output["module"], "twin");
    assert_eq!(output["weight"].as_f64(), Some(model.weight));
    assert_eq!(output["bias"].as_f64(), Some(model.bias));
}
