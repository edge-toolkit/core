//! Covers the probe set itself -- its shaders and correctly rounded answers -- and what a signature reads out of a
//! device's results: its counts, fingerprint, choices and per-probe lines.
#![cfg(test)]

use et_gpu_probe::{
    Answer, HOST, PROBE_COUNT, Signature, UNRECOGNISED, VERTEX_SHADER, WEBGL2, WEBGPU, combined_shader,
    fragment_shaders, host, reference, shaders,
};

/// The fingerprint the probes gave through WebGPU on an Apple M5 Pro.
const M5_PRO_WEBGPU: &str = concat!(
    "33800000-33800000-40800000-3eaaaaab-00000000-3fddb3d7-3f13cd3a-3dcc7576-",
    "3f7eb899-3dcd7c43-3f8d763e-3f8c9f54-3f8edd1f-3d087b99-3dcc1ebd-00000000"
);

/// A signature in which the device gave the correctly rounded answer to every probe.
fn correct() -> Signature {
    Signature::new(WEBGPU, reference().into_iter().map(Answer::Bits).collect())
}

/// The signature whose fingerprint is `fingerprint`, as measured through `interface`.
///
/// A word that is not hex, as `????????` is not, stands for a probe the device refused.
fn measured(interface: &'static str, fingerprint: &str) -> Signature {
    Signature::new(
        interface,
        fingerprint
            .split('-')
            .map(|word| {
                u32::from_str_radix(word, 16).map_or_else(|error| Answer::Refused(error.to_string()), Answer::Bits)
            })
            .collect(),
    )
}

/// The correctly rounded answers with the probe at `index` replaced by what `replacement` builds.
fn with<Build>(index: usize, replacement: Build) -> Signature
where
    Build: Fn() -> Answer,
{
    Signature::new(
        WEBGPU,
        reference()
            .into_iter()
            .enumerate()
            .map(|(position, bits)| {
                if position == index {
                    replacement()
                } else {
                    Answer::Bits(bits)
                }
            })
            .collect(),
    )
}

#[test]
fn every_probe_has_a_shader_of_its_own_writing_one_word() {
    let shaders = shaders();
    assert_eq!(shaders.len(), PROBE_COUNT);
    for (name, source) in &shaders {
        assert_ne!(*name, "");
        assert_eq!(source.matches("probeOut[0] = bitcast<u32>(").count(), 1);
        assert!(source.trim_end().ends_with('}'));
    }
    let mut names: Vec<&str> = shaders.iter().map(|shader| shader.0).collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), PROBE_COUNT);
}

#[test]
fn the_combined_shader_writes_every_probe_to_a_word_of_its_own() {
    let combined = combined_shader();
    assert!(combined.contains("probeOut : array<u32, 16>;"));
    for (word, (_, alone)) in shaders().iter().enumerate() {
        assert!(alone.contains("probeOut : array<u32, 1>;"));
        // The one write a probe's own shader makes is the write the combined shader makes to that probe's word.
        let write = alone.lines().find(|line| line.contains("probeOut[0] = ")).unwrap();
        assert!(combined.contains(&write.replace("probeOut[0]", &format!("probeOut[{word}]"))));
    }
    assert_eq!(combined.matches("] = bitcast<u32>(").count(), PROBE_COUNT);
}

#[test]
fn a_probe_shader_holds_its_own_expression() {
    let shaders = shaders();
    assert!(
        shaders
            .iter()
            .any(|shader| shader.0 == "fma" && shader.1.contains("fma(ulpBase, ulpBase, negatedSquare)"))
    );
}

#[test]
fn every_probe_has_a_fragment_shader_spelled_for_glsl() {
    let fragments = fragment_shaders();
    assert_eq!(fragments.len(), PROBE_COUNT);
    for (name, source) in &fragments {
        assert_ne!(*name, "");
        assert!(source.starts_with("#version 300 es"));
        assert_eq!(source.matches("uint bits = floatBitsToUint(").count(), 1);
        // WGSL spellings that GLSL ES 3.00 does not have.
        assert!(!source.contains("inverseSqrt") && !source.contains("atan2") && !source.contains("vec4<f32>"));
    }
    assert!(VERTEX_SHADER.starts_with("#version 300 es"));
}

#[test]
fn the_exact_probes_have_their_strict_answers() {
    let bits = reference();
    // multiply_add rounded twice, fma fused to 2^-24, the sum added in order, 1/3, 2^-127, sqrt(3), and the dot.
    assert_eq!(
        [bits[0], bits[1], bits[2], bits[3], bits[4], bits[5], bits[15]],
        [0, 0x3380_0000, 0, 0x3eaa_aaab, 0x0040_0000, 0x3fdd_b3d7, 0]
    );
}

#[test]
fn the_function_probes_are_within_one_ulp_of_the_host_f32_functions() {
    for (correct, near) in reference().into_iter().zip(host()) {
        assert!(correct.abs_diff(near) <= 1);
    }
}

#[test]
fn the_host_gives_the_exact_probes_their_strict_answers() {
    let signature = Signature::new(HOST, host().into_iter().map(Answer::Bits).collect());
    assert_eq!(signature.completed(), PROBE_COUNT);
    assert_eq!(signature.traits(), correct().traits());
}

#[test]
fn a_device_giving_every_correct_answer_deviates_nowhere() {
    let signature = correct();
    assert_eq!(signature.completed(), PROBE_COUNT);
    assert_eq!(signature.deviations(), 0);
    assert_eq!(
        signature.traits(),
        "multiply_add=rounded_twice fma=fused swamped_sum=in_order subnormal_product=kept dot=in_order"
    );
}

#[test]
fn a_fingerprint_is_every_result_as_a_hex_word() {
    let fingerprint = correct().fingerprint();
    assert!(fingerprint.starts_with("00000000-33800000-00000000-3eaaaaab-00400000-3fddb3d7-"));
    assert_eq!(fingerprint.split('-').count(), PROBE_COUNT);
}

#[test]
fn a_fused_multiply_add_is_a_deviation_named_as_a_choice() {
    let signature = with(0, || Answer::Bits(0x3380_0000));
    assert_eq!(signature.deviations(), 1);
    assert!(signature.traits().starts_with("multiply_add=fused "));
    assert!(
        signature
            .describe()
            .iter()
            .any(|line| line == "multiply_add gpu=33800000 correct=00000000 ulps=+864026624")
    );
}

#[test]
fn an_answer_that_is_neither_choice_reads_other() {
    assert!(
        with(2, || Answer::Bits(0x3f80_0000))
            .traits()
            .contains("swamped_sum=other")
    );
}

#[test]
fn a_probe_the_device_refused_is_a_gap_not_a_deviation() {
    let signature = with(1, || Answer::Refused("shader rejected".to_owned()));
    assert_eq!(signature.completed(), 15);
    assert_eq!(signature.deviations(), 0);
    assert!(signature.fingerprint().starts_with("00000000-????????-00000000-"));
    assert!(signature.traits().contains(" fma=failed "));
    assert!(
        signature
            .describe()
            .iter()
            .any(|line| line == "fma failed: shader rejected")
    );
}

#[test]
fn a_device_that_answered_nothing_fails_every_choice() {
    let signature = Signature::new(WEBGPU, Vec::new());
    assert_eq!(signature.completed(), 0);
    assert_eq!(
        signature.traits(),
        "multiply_add=failed fma=failed swamped_sum=failed subnormal_product=failed dot=failed"
    );
    assert_eq!(signature.describe(), Vec::<String>::new());
}

#[test]
fn answers_no_known_gpu_gives_are_unrecognised() {
    // Too few answers to be anyone's, then a full set that is nobody's.
    assert_eq!(Signature::new(WEBGPU, vec![Answer::Bits(1)]).class(), UNRECOGNISED);
    assert_eq!(correct().class(), UNRECOGNISED);
}

#[test]
fn a_known_gpus_answers_through_another_interface_are_unrecognised() {
    assert_eq!(measured(WEBGL2, M5_PRO_WEBGPU).class(), UNRECOGNISED);
}

#[test]
fn a_known_signature_can_hold_a_probe_the_device_refused() {
    // WebGL 2 has no `fma`, so the M5 Pro's signature through it has a gap where that answer would be.
    let signature = measured(
        WEBGL2,
        concat!(
            "33800000-????????-40800000-3eaaaaab-00000000-3fddb3d7-3f13cd3a-3dcc7576-",
            "3f7eb899-3dcd7c43-3f8d763e-3f8c9f54-3f8edd1f-3d087b99-3dcc1ebc-00000000"
        ),
    );
    assert_eq!(signature.completed(), 15);
    assert_eq!(
        signature.class(),
        "Apple silicon GPU through Metal (measured on an Apple M5 Pro)"
    );
}

#[test]
fn the_answers_measured_on_an_apple_m5_pro_are_recognised() {
    let signature = measured(WEBGPU, M5_PRO_WEBGPU);
    assert_eq!(signature.fingerprint(), M5_PRO_WEBGPU);
    assert_eq!(
        signature.class(),
        "Apple silicon GPU through Metal (measured on an Apple M5 Pro)"
    );
    assert_eq!(signature.deviations(), 8);
    assert_eq!(
        signature.traits(),
        "multiply_add=fused fma=fused swamped_sum=reassociated subnormal_product=flushed dot=in_order"
    );
}

#[test]
fn the_registry_finds_a_browsers_signature_where_each_probe_had_a_shader_of_its_own() {
    let labels = measured(WEBGPU, M5_PRO_WEBGPU).identify();
    assert_ne!(labels, Vec::<&str>::new());
    for label in labels {
        assert!(label.contains("Apple M5 Pro"));
    }
}

#[test]
fn the_registry_finds_a_signature_filed_under_the_interface_itself() {
    let correct = Signature::new(HOST, reference().into_iter().map(Answer::Bits).collect());
    assert_eq!(correct.identify().len(), 1);
}

#[test]
fn the_registry_does_not_find_a_signature_through_another_interface() {
    assert_eq!(measured(WEBGL2, M5_PRO_WEBGPU).identify(), Vec::<&str>::new());
}

#[test]
fn the_registry_has_no_label_for_a_signature_it_has_not_seen() {
    assert_eq!(correct().identify(), Vec::<&str>::new());
}

#[test]
fn the_registry_matches_nothing_to_a_signature_missing_answers() {
    assert_eq!(
        Signature::new(HOST, vec![Answer::Bits(0)]).identify(),
        Vec::<&str>::new()
    );
    // No answers at all is what an entry kept for a withheld interface holds, and still names nothing.
    assert_eq!(Signature::new(WEBGPU, Vec::new()).identify(), Vec::<&str>::new());
}
