//! The math1 family's Rust: the `FedAvg` kernel, and the storage protocol its twins and triggers speak.
//!
//! A trigger stores the canonical input JSON as [`INPUT_FILENAME`] in its own bucket and relays the frame
//! [`pointer_frame`] builds, naming it. Each twin waits for that frame, reads the input from storage, runs
//! [`fed_avg`], and stores what [`output_json`] renders as [`OUTPUT_FILENAME`] in its own bucket. The kernel uses only
//! `+ - * /` on f64 in a fixed order, so every runtime that follows IEEE 754 computes the same model to the bit, the
//! twins in other languages included.
//!
//! `no_std` with `alloc`, so a twin built for a target with no standard library uses it as a browser or WASI one does.

#![no_std]
#![expect(clippy::float_arithmetic, reason = "the FedAvg kernel is float math by design")]

extern crate alloc;

use alloc::string::{String, ToString as _};
use alloc::vec::Vec;

use serde::Deserialize;
use serde_json::{Value, json};

/// The `type` of the pointer frame a trigger relays.
pub const INPUT_TYPE: &str = "math1-input";
/// Storage object name of the canonical input, inside the trigger's bucket.
pub const INPUT_FILENAME: &str = "math1-input.json";
/// Storage object name each twin stores its global model as, inside its own bucket.
pub const OUTPUT_FILENAME: &str = "math1-output.json";

/// The canonical input: per-client (feature, target) samples plus the training hyperparameters.
#[derive(Debug, Deserialize)]
#[non_exhaustive]
pub struct Math1Input {
    /// Each client's (feature, target) samples.
    pub clients: Vec<Vec<(f64, f64)>>,
    /// How many rounds of local training and merging to run.
    pub rounds: u32,
    /// How many full-batch gradient-descent epochs each client runs per round.
    pub epochs: u32,
    /// The gradient-descent step size.
    pub learning_rate: f64,
}

/// Where a trigger stored the input: the bucket and file a pointer frame names.
#[derive(Clone, Debug, Deserialize)]
#[non_exhaustive]
pub struct InputPointer {
    /// The trigger's bucket.
    pub bucket: String,
    /// The input's name in that bucket.
    pub filename: String,
}

/// Why a text frame is not the math1 input pointer.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PointerError {
    /// The frame is not JSON, or claims to be the pointer but does not name a bucket and file.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    /// The frame is JSON, but a message other than the pointer.
    #[error("not a math1-input frame")]
    OtherType,
}

/// A global model, fitting `target = weight * feature + bias`.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct Model {
    /// The fitted weight.
    pub weight: f64,
    /// The fitted bias.
    pub bias: f64,
}

/// The pointer frame a trigger relays, naming [`INPUT_FILENAME`] in `bucket`.
#[must_use]
pub fn pointer_frame(bucket: &str) -> String {
    json!({ "type": INPUT_TYPE, "bucket": bucket, "filename": INPUT_FILENAME }).to_string()
}

/// The input pointer the text frame `text` carries.
///
/// # Errors
///
/// When `text` is not JSON, is a message other than the pointer, or is a pointer naming no bucket and file.
pub fn parse_pointer(text: &str) -> Result<InputPointer, PointerError> {
    let value: Value = serde_json::from_str(text)?;
    if value.get("type").and_then(Value::as_str) != Some(INPUT_TYPE) {
        return Err(PointerError::OtherType);
    }
    Ok(serde_json::from_value(value)?)
}

/// Sample count as f64, accumulated additively to avoid an integer-to-float cast.
fn sample_count(samples: &[(f64, f64)]) -> f64 {
    samples.iter().fold(0.0_f64, |count, _| count + 1.0_f64)
}

/// Run the `FedAvg` simulation on `input`, returning the final global model.
///
/// Each round, every client starts from the global model, runs its local epochs of full-batch gradient descent, and
/// the results are merged as an average weighted by each client's sample count.
#[must_use]
pub fn fed_avg(input: &Math1Input) -> Model {
    let mut weight = 0.0_f64;
    let mut bias = 0.0_f64;
    let total_samples: f64 = input
        .clients
        .iter()
        .fold(0.0_f64, |acc, samples| acc + sample_count(samples));
    for _ in 0_u32..input.rounds {
        let mut merged_weight = 0.0_f64;
        let mut merged_bias = 0.0_f64;
        for samples in &input.clients {
            let count = sample_count(samples);
            let mut client_weight = weight;
            let mut client_bias = bias;
            for _ in 0_u32..input.epochs {
                let mut grad_weight = 0.0_f64;
                let mut grad_bias = 0.0_f64;
                for &(feature, target) in samples {
                    let residual = client_weight * feature + client_bias - target;
                    grad_weight += residual * feature;
                    grad_bias += residual;
                }
                client_weight -= input.learning_rate * (2.0_f64 * grad_weight / count);
                client_bias -= input.learning_rate * (2.0_f64 * grad_bias / count);
            }
            merged_weight += client_weight * count;
            merged_bias += client_bias * count;
        }
        weight = merged_weight / total_samples;
        bias = merged_bias / total_samples;
    }
    Model { weight, bias }
}

/// The JSON a twin stores as [`OUTPUT_FILENAME`]: the name of the `module` that computed `model`, and the model.
#[must_use]
pub fn output_json(module: &str, model: Model) -> String {
    json!({ "module": module, "weight": model.weight, "bias": model.bias }).to_string()
}
