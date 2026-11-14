//! The GPU probe sets and canvas fingerprints as WASM, for a page to put to the browser's WebGL 2 with
//! [`probe_webgl2`], to each of its WebGPU adapters with [`probe_webgpu`], and to its 2-D canvas with [`probe_canvas`].
#![cfg_attr(feature = "docs", doc = document_features::document_features!())]
#![cfg_attr(
    target_arch = "wasm32",
    expect(
        clippy::future_not_send,
        reason = "a page's WebGPU objects are JavaScript's, which stay on the one thread a page has"
    )
)]

use et_gpu_probe::Signature;
use et_gpu_probe::native::Gpu;
// rustc links only the crates a crate names, and the coverage build needs et-web's profiler runtime linked.
#[cfg(all(feature = "coverage", target_arch = "wasm32"))]
use et_web as _;
use serde_json::{Value, json};
use wasm_bindgen::prelude::{JsError, wasm_bindgen};

mod batch;
mod canvas;
mod targets;
mod webgl1;
mod webgl2;

/// Take both canvas fingerprints, and give back what each answered.
///
/// The answer is JSON text: an object whose `signatures` hold each fingerprint the browser answered the same way
/// every time it was read, as [`probe_webgl2`] gives them, whose `withheld` hold each it answered with noise, by its
/// `interface`, `reason` and `describe` lines, whose `report` says, a line each, how every way the page makes and reads
/// a canvas gave back a scene whose values are all known, and whose `problems` say why one could not be taken at all.
#[wasm_bindgen]
pub fn probe_canvas() -> Result<String, JsError> {
    let mut signatures = Vec::new();
    let mut withheld = Vec::new();
    let mut problems = Vec::new();
    let paths = canvas::read_paths()?;
    for (interface, outcome) in [
        (et_gpu_probe::CANVAS, canvas::scene(&paths)),
        (et_gpu_probe::CANVAS_BROWSERLEAKS, canvas::browserleaks(&paths)),
    ] {
        match outcome {
            Ok(answer) if answer.get("reason").is_some() => withheld.push(answer),
            Ok(answer) => signatures.push(answer),
            Err(problem) => problems.push(format!("{interface}: {problem}")),
        }
    }
    let answer = json!({
        "signatures": signatures,
        "withheld": withheld,
        "report": paths.into_lines(),
        "problems": problems,
    });
    Ok(serde_json::to_string(&answer)?)
}

/// Put every probe set WebGL 2 can run to the browser's WebGL 2, and give back what each answered.
///
/// The answer is JSON text: an object whose `signatures` hold one object a probe set, and whose `problems` say why a
/// set gave nothing. Each signature gives the `interface` the registry files it under, its `results` as `[name, word]`
/// pairs in output order, and its `fingerprint`, `traits` and `describe` lines.
#[wasm_bindgen]
pub fn probe_webgl2() -> Result<String, JsError> {
    let (signatures, problems) = gather([
        ("webgl2-split", webgl2::signature()),
        (et_gpu_probe::WEBGL2_COMBINED, webgl2::combined_signature()),
        ("webgl2-clear", targets::clear_signature()),
        ("webgl2-fixed-function", targets::fixed_function_signature()),
    ]);
    let answer = json!({ "signatures": signatures, "problems": problems });
    Ok(serde_json::to_string(&answer)?)
}

/// Put the probe set to the browser's WebGL 1, a shader a probe and then all in one, and give back what each answered.
///
/// The answer is JSON text as [`probe_webgl2`] gives it. A browser that gives no WebGL 1, or none with highp float in
/// its fragment shaders, answers with one problem saying so.
#[wasm_bindgen]
pub fn probe_webgl1() -> Result<String, JsError> {
    let (signatures, problems) = gather(webgl1::signatures());
    let answer = json!({ "signatures": signatures, "problems": problems });
    Ok(serde_json::to_string(&answer)?)
}

/// Put every probe set to the WebGPU adapter the browser gives for `power` and `fallback`, and give back the answers.
///
/// `power` is a `GPUPowerPreference`, `high-performance` or `low-power`, and `fallback` asks for the browser's fallback
/// adapter, a software renderer, instead. The answer is JSON text as [`probe_webgl2`] gives it, with a `report` beside
/// the signatures: the adapter's description, one `name: value` line each.
#[wasm_bindgen]
pub async fn probe_webgpu(power: &str, fallback: bool) -> Result<String, JsError> {
    let power_preference = match power {
        "high-performance" => wgpu::PowerPreference::HighPerformance,
        "low-power" => wgpu::PowerPreference::LowPower,
        _ => wgpu::PowerPreference::None,
    };
    let options = wgpu::RequestAdapterOptions {
        power_preference,
        force_fallback_adapter: fallback,
        compatible_surface: None,
    };
    let (signatures, report, problems) = match Gpu::open_with(&options).await {
        Ok(gpu) => {
            // Each set is measured on its own, so one that fails -- the device lost at its readback, say -- leaves the
            // sets before it answered and submittable, and is reported beside them.
            let (signatures, problems) = gather([
                ("webgpu-split", gpu.signature().await),
                ("webgpu-combined", gpu.combined_signature().await),
                ("webgpu-clear", gpu.clear_signature().await),
                ("webgpu-fixed-function", gpu.fixed_function_signature().await),
            ]);
            (signatures, gpu.report().lines, problems)
        }
        Err(problem) => (Vec::new(), Vec::new(), vec![format!("WebGPU: {problem}")]),
    };
    let answer = json!({ "signatures": signatures, "report": report, "problems": problems });
    Ok(serde_json::to_string(&answer)?)
}

/// What a page shows of each set in `measured` that answered, and why each that did not gave nothing.
///
/// A set that answered is given by the `interface` it was measured through, its `results` as `[name, word]` pairs, and
/// its `fingerprint`, `traits` and `describe` lines.
fn gather<Problem, Sets>(measured: Sets) -> (Vec<Value>, Vec<String>)
where
    Problem: core::fmt::Display,
    Sets: IntoIterator<Item = (&'static str, Result<Signature, Problem>)>,
{
    let mut signatures = Vec::new();
    let mut problems = Vec::new();
    for (interface, outcome) in measured {
        match outcome {
            Ok(signature) => signatures.push(json!({
                "interface": interface,
                "results": signature.results(),
                "fingerprint": signature.fingerprint(),
                "traits": signature.traits(),
                "describe": signature.describe(),
            })),
            Err(problem) => problems.push(format!("{interface}: {problem}")),
        }
    }
    (signatures, problems)
}
