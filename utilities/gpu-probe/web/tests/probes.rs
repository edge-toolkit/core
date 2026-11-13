//! Puts every probe set and both canvas fingerprints to a real browser's WebGL 2, WebGPU and 2-D canvas, and checks the
//! page gets back the JSON it reads: signatures with their results, and a reason for whatever was not probed.
#![cfg(test)]
#![cfg(target_arch = "wasm32")]
#![expect(
    clippy::future_not_send,
    clippy::single_call_fn,
    reason = "each test is called once, by the wrapper #[wasm_bindgen_test] makes; the WebGPU probe holds page objects"
)]
#![cfg_attr(wasm_bindgen_unstable_test_coverage, feature(coverage_attribute))]

use et_int_gpu_probe_web::{probe_canvas, probe_webgl2, probe_webgpu};
use serde_json::Value;
use wasm_bindgen_test::wasm_bindgen_test;

wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

/// The JSON a probe gave, parsed.
fn parsed(answer: &str) -> Value {
    serde_json::from_str(answer).unwrap()
}

/// The `interface` of each signature in `answer`.
fn interfaces(answer: &Value) -> Vec<&str> {
    answer["signatures"]
        .as_array()
        .unwrap()
        .iter()
        .map(|signature| signature["interface"].as_str().unwrap())
        .collect()
}

#[wasm_bindgen_test]
fn webgl2_gives_every_set_or_says_why_not() {
    let answer = parsed(&probe_webgl2().unwrap());
    let given = interfaces(&answer);
    let problems = answer["problems"].as_array().unwrap();
    assert_eq!(given.len().saturating_add(problems.len()), 3, "{answer}");
    for signature in answer["signatures"].as_array().unwrap() {
        assert!(!signature["results"].as_array().unwrap().is_empty(), "{signature}");
        assert!(signature["describe"].is_array(), "{signature}");
    }
}

#[wasm_bindgen_test]
async fn webgpu_gives_every_set_or_says_why_not() {
    for (power, fallback) in [("high-performance", false), ("low-power", false), ("", true)] {
        let answer = parsed(&probe_webgpu(power, fallback).await.unwrap());
        let given = interfaces(&answer);
        let problems = answer["problems"].as_array().unwrap();
        assert!(given.len() == 4 || !problems.is_empty(), "{answer}");
        assert!(answer["report"].is_array(), "{answer}");
    }
}

#[wasm_bindgen_test]
fn both_canvas_fingerprints_are_taken() {
    let answer = parsed(&probe_canvas().unwrap());
    assert_eq!(answer["problems"].as_array().unwrap().len(), 0, "{answer}");
    let mut taken: Vec<&str> = interfaces(&answer);
    for withheld in answer["withheld"].as_array().unwrap() {
        assert!(
            withheld["reason"].as_str().unwrap().starts_with("randomised: "),
            "{withheld}"
        );
        taken.push(withheld["interface"].as_str().unwrap());
    }
    taken.sort_unstable();
    assert_eq!(taken, ["canvas", "canvas-browserleaks"]);
    let report = answer["report"].as_array().unwrap();
    assert_eq!(report.len(), 8, "{answer}");
}

#[wasm_bindgen_test]
fn an_unaltered_browser_gives_the_hashes_its_reads_agree_on() {
    let answer = parsed(&probe_canvas().unwrap());
    for signature in answer["signatures"].as_array().unwrap() {
        let describe = signature["describe"].as_array().unwrap();
        assert!(describe.len() >= 2, "{signature}");
        let fingerprint = signature["fingerprint"].as_str().unwrap();
        assert!(
            describe.iter().all(|line| line.as_str().unwrap().contains(fingerprint)),
            "{signature}"
        );
    }
}
