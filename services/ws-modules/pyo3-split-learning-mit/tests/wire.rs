//! Wire-format round-trip tests. These don't touch PyO3 or PyTorch — they
//! just verify the JSON+base64 envelope codec stays in sync with the demo's
//! `split_learning.schemas.message.WSMessage` shape.

use std::collections::BTreeMap;

use et_ws_pyo3_split_learning_mit::wire::{
    InboundKind, RawEnvelope, decode_inbound, encode_grads, inner_base64_encode, outer_base64_decode,
    outer_base64_encode,
};

#[test]
fn encode_decode_envelope_round_trip() {
    let tensor_bytes = vec![0u8, 1, 2, 3, 4];
    let shape = vec![1i64, 5];
    let encoded = encode_grads(&tensor_bytes, &shape, 0.42).expect("encode");

    let decoded_b64 = outer_base64_decode(&encoded).expect("outer base64");
    let json_text = String::from_utf8(decoded_b64).expect("utf-8");
    let envelope: RawEnvelope = serde_json::from_str(&json_text).expect("json");
    assert_eq!(envelope.type_, "grads");

    let tensor_inner =
        base64::Engine::decode(&base64::engine::general_purpose::STANDARD, envelope.raw["tensor"].as_bytes())
            .expect("inner base64");
    assert_eq!(tensor_inner, tensor_bytes);

    let data = envelope.data.expect("data");
    assert_eq!(data["loss"], serde_json::json!(0.42));
    assert_eq!(data["tensor_shape"], serde_json::json!([1, 5]));
}

#[test]
fn decode_inbound_activations_only() {
    // Build a synthetic inbound `activations` frame the same way the demo
    // client would: JSON envelope, then outer base64.
    let tensor = [9u8, 9, 9, 9];
    let raw = BTreeMap::from([("tensor".to_string(), inner_base64_encode(&tensor))]);
    let envelope = RawEnvelope {
        type_: "activations".to_string(),
        data: Some(serde_json::json!({ "tensor_shape": [1, 4] })),
        raw,
    };
    let json = serde_json::to_string(&envelope).unwrap();
    let frame = outer_base64_encode(json.as_bytes());

    let parsed = decode_inbound(&frame).expect("decode");
    assert_eq!(parsed.kind, InboundKind::Activations);
    assert_eq!(parsed.tensor_shape, vec![1, 4]);
    assert_eq!(parsed.tensor_bytes, tensor);
    assert!(parsed.labels_bytes.is_none());
}

#[test]
fn decode_inbound_activations_and_labels() {
    let tensor = [1u8; 8];
    let labels = [2u8; 8];
    let raw = BTreeMap::from([
        ("tensor".to_string(), inner_base64_encode(&tensor)),
        ("labels".to_string(), inner_base64_encode(&labels)),
    ]);
    let envelope = RawEnvelope {
        type_: "activations_and_labels".to_string(),
        data: Some(serde_json::json!({ "tensor_shape": [1, 2] })),
        raw,
    };
    let json = serde_json::to_string(&envelope).unwrap();
    let frame = outer_base64_encode(json.as_bytes());

    let parsed = decode_inbound(&frame).expect("decode");
    assert_eq!(parsed.kind, InboundKind::ActivationsAndLabels);
    assert_eq!(parsed.tensor_bytes, tensor);
    assert_eq!(parsed.labels_bytes.as_deref(), Some(&labels[..]));
}
