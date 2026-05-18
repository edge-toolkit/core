//! Wire-format adapter for the MIT split-learning-demo.
//!
//! The demo's WebSocket frames are *binary* frames whose payload is a
//! base64-encoded UTF-8 JSON document. Inside the JSON, the `raw` map holds
//! per-tensor blobs that are themselves base64-encoded.
//!
//! Doing both base64 layers + JSON in Rust keeps the PyO3 boundary narrow:
//! Python only ever sees raw `bytes` and `(tensor_shape, [labels])` tuples.

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, thiserror::Error)]
pub enum WireError {
    #[error("frame is not valid base64: {0}")]
    Base64(#[from] base64::DecodeError),
    #[error("decoded payload is not valid UTF-8: {0}")]
    Utf8(#[from] std::string::FromUtf8Error),
    #[error("json envelope: {0}")]
    Json(#[from] serde_json::Error),
    #[error("missing `{0}` field in payload")]
    MissingField(&'static str),
    #[error("`{field}` has unexpected type ({reason})")]
    BadField {
        field: &'static str,
        reason: &'static str,
    },
}

/// Subset of the demo's `MessageType` enum we actually handle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InboundKind {
    /// Training step input: smashed activations + ground-truth labels.
    ActivationsAndLabels,
    /// Inference step input: activations only.
    Activations,
}

impl InboundKind {
    fn parse(s: &str) -> Option<Self> {
        match s {
            "activations_and_labels" => Some(Self::ActivationsAndLabels),
            "activations" => Some(Self::Activations),
            _ => None,
        }
    }
}

#[derive(Debug)]
pub struct InboundMessage {
    pub kind: InboundKind,
    pub tensor_shape: Vec<i64>,
    pub tensor_bytes: Vec<u8>,
    /// Only populated for `ActivationsAndLabels`.
    pub labels_bytes: Option<Vec<u8>>,
}

/// Parse a binary WebSocket payload from the demo client into a typed view.
///
/// The frame body is `base64(utf8(json({type, data, raw})))`. `raw["tensor"]`
/// and `raw["labels"]` are themselves base64-encoded blobs.
pub fn decode_inbound(frame: &[u8]) -> Result<InboundMessage, WireError> {
    let decoded = BASE64.decode(frame)?;
    let json_text = String::from_utf8(decoded)?;
    let envelope: RawEnvelope = serde_json::from_str(&json_text)?;

    let kind = InboundKind::parse(&envelope.type_).ok_or(WireError::BadField {
        field: "type",
        reason: "expected `activations` or `activations_and_labels`",
    })?;

    let tensor_shape_value = envelope
        .data
        .as_ref()
        .and_then(|d| d.get("tensor_shape"))
        .ok_or(WireError::MissingField("data.tensor_shape"))?;
    let tensor_shape = parse_shape(tensor_shape_value)?;

    let tensor_b64 = envelope.raw.get("tensor").ok_or(WireError::MissingField("raw.tensor"))?;
    let tensor_bytes = BASE64.decode(tensor_b64.as_bytes())?;

    let labels_bytes = match kind {
        InboundKind::ActivationsAndLabels => {
            let labels_b64 = envelope
                .raw
                .get("labels")
                .ok_or(WireError::MissingField("raw.labels"))?;
            Some(BASE64.decode(labels_b64.as_bytes())?)
        }
        InboundKind::Activations => None,
    };

    Ok(InboundMessage {
        kind,
        tensor_shape,
        tensor_bytes,
        labels_bytes,
    })
}

/// Build the binary frame the demo client expects for a training response.
pub fn encode_grads(tensor_bytes: &[u8], tensor_shape: &[i64], loss: f64) -> Result<Vec<u8>, WireError> {
    let mut data = serde_json::Map::new();
    data.insert("tensor_shape".into(), shape_to_json(tensor_shape));
    data.insert("loss".into(), serde_json::Value::from(loss));
    encode_envelope("grads", data, tensor_bytes)
}

/// Build the binary frame the demo client expects for an inference response.
pub fn encode_logits(tensor_bytes: &[u8], tensor_shape: &[i64]) -> Result<Vec<u8>, WireError> {
    let mut data = serde_json::Map::new();
    data.insert("tensor_shape".into(), shape_to_json(tensor_shape));
    encode_envelope("logits", data, tensor_bytes)
}

fn encode_envelope(
    type_: &str,
    data: serde_json::Map<String, serde_json::Value>,
    tensor_bytes: &[u8],
) -> Result<Vec<u8>, WireError> {
    let mut raw = BTreeMap::new();
    raw.insert("tensor".to_string(), BASE64.encode(tensor_bytes));
    let envelope = RawEnvelope {
        type_: type_.to_string(),
        data: Some(serde_json::Value::Object(data)),
        raw,
    };
    let json_text = serde_json::to_string(&envelope)?;
    Ok(BASE64.encode(json_text).into_bytes())
}

fn parse_shape(value: &serde_json::Value) -> Result<Vec<i64>, WireError> {
    let arr = value.as_array().ok_or(WireError::BadField {
        field: "data.tensor_shape",
        reason: "expected JSON array",
    })?;
    arr.iter()
        .map(|v| {
            v.as_i64().ok_or(WireError::BadField {
                field: "data.tensor_shape",
                reason: "expected integer dimensions",
            })
        })
        .collect()
}

fn shape_to_json(shape: &[i64]) -> serde_json::Value {
    serde_json::Value::Array(shape.iter().map(|d| serde_json::Value::from(*d)).collect())
}

/// Server's wire envelope. Public so test code can re-parse encoded frames
/// without depending on the demo's Python package being present.
#[derive(Debug, Serialize, Deserialize)]
pub struct RawEnvelope {
    #[serde(rename = "type")]
    pub type_: String,
    #[serde(default)]
    pub data: Option<serde_json::Value>,
    /// Per-tensor base64-encoded blobs. The demo's `WSMessage` serialises
    /// `raw: dict[str, bytes]` by base64-encoding the bytes (see
    /// `split_learning.schemas.message.WSMessage.Config.json_encoders`).
    #[serde(default)]
    pub raw: BTreeMap<String, String>,
}

/// Hand to tests so they can use the same outer base64 the wire helpers do.
pub fn outer_base64_decode(frame: &[u8]) -> Result<Vec<u8>, base64::DecodeError> {
    BASE64.decode(frame)
}

/// Hand to tests so they can build a synthetic inbound frame.
pub fn outer_base64_encode(bytes: &[u8]) -> Vec<u8> {
    BASE64.encode(bytes).into_bytes()
}

/// Inner base64 used for `raw.<key>` blobs inside the JSON envelope.
pub fn inner_base64_encode(bytes: &[u8]) -> String {
    BASE64.encode(bytes)
}
