//! Verify a stored `math1-output.json` read from stdin against the canonical expected model.
//!
//! The `k3s-verify` task pulls that file out of a running cluster with `kubectl exec` and pipes it here, so the
//! numeric expectation stays in [`et_ws_test_server::math1`] with every other consumer of it rather than being
//! retyped as a shell comparison that could drift from the constants it mirrors.
//!
//! Reads stdin rather than taking a path, because the file being checked lives inside a pod: there is nothing on
//! this filesystem to name, and `kubectl exec ... | verify-math1` needs no temporary file.

use std::error::Error;
use std::io::Read as _;

#[expect(
    clippy::print_stdout,
    reason = "a verification CLI reports its result on stdout; that is its whole output"
)]
fn main() -> Result<(), Box<dyn Error>> {
    let mut stored = String::default();
    let _bytes = std::io::stdin().read_to_string(&mut stored)?;

    // Matched rather than `?`, so the failure quotes the bytes that came out of the pod. A bare `?` would
    // report only serde's position, which says nothing about what the cluster actually stored.
    let value: serde_json::Value = match serde_json::from_str(stored.trim()) {
        Ok(value) => value,
        Err(error) => return Err(format!("stored model is not JSON: {error}: {stored}").into()),
    };
    let weight = value
        .get("weight")
        .and_then(serde_json::Value::as_f64)
        .ok_or_else(|| format!("stored model has no numeric `weight`: {stored}"))?;
    let bias = value
        .get("bias")
        .and_then(serde_json::Value::as_f64)
        .ok_or_else(|| format!("stored model has no numeric `bias`: {stored}"))?;

    et_ws_test_server::math1::verify_math1_model(weight, bias)?;
    println!("math1 model verified: weight={weight} bias={bias}");

    Ok(())
}
