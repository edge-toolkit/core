//! Covers the `f16` probe set: its shaders, its correctly rounded answers, and what a signature measured through it
//! reads out of a device's results.
#![cfg(test)]

use et_gpu_probe::{
    Answer, F16_INPUTS, PROBE_COUNT, Signature, UNRECOGNISED, WEBGPU, WEBGPU_F16, WEBGPU_F16_COMBINED,
    f16_combined_shader, f16_reference, f16_shaders, shaders,
};
use half::f16;
use pretty_assertions::assert_eq;
use pretty_assertions::assert_ne;

/// Where in output order the probes of mathematical functions start.
const FIRST_FUNCTION: usize = 5;

/// What the `f16` probes answer, by name, on a device computing `f16` exactly as written.
fn correct(interface: &'static str) -> Signature {
    Signature::new(interface, f16_reference().into_iter().map(Answer::Bits).collect())
}

#[test]
fn every_f16_probe_has_a_shader_of_its_own_needing_shader_f16() {
    let f16_shaders = f16_shaders();
    assert_eq!(f16_shaders.len(), PROBE_COUNT);
    for ((name, shader), (f32_name, _)) in f16_shaders.iter().zip(shaders()) {
        assert_eq!(*name, f32_name);
        assert!(shader.starts_with("enable f16;\n"), "{shader}");
        assert!(shader.contains("array<vec2<f16>, 1>"), "{shader}");
        assert!(shader.contains("probeOut[0] = vec2<f16>("), "{shader}");
    }
    let multiply_add = &f16_shaders.first().unwrap().1;
    assert!(multiply_add.contains("left * right + negatedProduct"), "{multiply_add}");
}

#[test]
fn the_combined_f16_shader_writes_every_probe_to_a_word_of_its_own() {
    let combined = f16_combined_shader();
    assert!(combined.starts_with("enable f16;\n"));
    assert!(combined.contains(&format!("array<vec2<f16>, {PROBE_COUNT}>")));
    assert!(combined.contains(&format!("probeOut[{}] =", PROBE_COUNT - 1)));
}

#[test]
fn every_f16_input_is_a_value_f16_holds_exactly() {
    for input in F16_INPUTS {
        assert_eq!(f16::from_f32(input).to_f32().to_bits(), input.to_bits());
    }
}

#[test]
fn the_exact_f16_probes_have_their_strict_answers() {
    let reference = f16_reference();
    // multiply_add rounds twice to 0, fma keeps 2^-11, the swamped sum loses every one, 1/3 rounds to 0x3555, the
    // halved smallest normal is the subnormal 2^-15, and the dot product loses both ones.
    let exact = [
        (0, 0),
        (1, 0x1000),
        (2, 0),
        (3, 0x3555),
        (4, 0x0200),
        (PROBE_COUNT - 1, 0),
    ];
    for (index, bits) in exact {
        assert_eq!(*reference.get(index).unwrap(), bits, "probe {index}");
    }
}

#[test]
fn the_f16_function_probes_are_within_one_ulp_of_the_f32_functions_rounded_to_f16() {
    let [_, _, _, _, _, angle, three, _] = F16_INPUTS;
    let functions = [
        f32::sqrt(three),
        f32::sqrt(three).recip(),
        f32::sin(angle),
        f32::cos(angle),
        f32::tan(angle),
        f32::exp(angle),
        f32::ln(three),
        f32::powf(three, angle),
        f32::atan2(angle, three),
        f32::tanh(angle),
    ];
    for (correct, function) in f16_reference().iter().skip(FIRST_FUNCTION).zip(functions) {
        let rounded = u32::from(f16::from_f32(function).to_bits());
        assert!(correct.abs_diff(rounded) <= 1, "{correct:04x} against {rounded:04x}");
    }
}

#[test]
fn a_device_computing_f16_as_written_deviates_nowhere() {
    let signature = correct(WEBGPU_F16);
    assert_eq!(signature.completed(), PROBE_COUNT);
    assert_eq!(signature.deviations(), 0);
    assert_eq!(
        signature.traits(),
        "multiply_add=rounded_twice fma=fused swamped_sum=in_order subnormal_product=kept dot=in_order"
    );
    assert_eq!(signature.class(), UNRECOGNISED);
    assert_eq!(
        signature.results().first().unwrap(),
        &("multiply_add", "00000000".to_owned())
    );
    let subnormal = "subnormal_product gpu=00000200 correct=00000200 ulps=+0".to_owned();
    assert!(signature.describe().contains(&subnormal));
}

#[test]
fn a_device_widening_f16_to_f32_reads_as_fused_and_regrouped() {
    // A widening device keeps the product's 2^-11, the four ones as 4, no subnormal, and both ones of the dot product.
    let replaced = [(0, 0x1000), (2, 0x4400), (4, 0), (PROBE_COUNT - 1, 0x4000)];
    let bits = f16_reference()
        .into_iter()
        .enumerate()
        .map(|(index, bits)| {
            let replacement = replaced.iter().find(|pair| pair.0 == index);
            Answer::Bits(replacement.map_or(bits, |pair| pair.1))
        })
        .collect();
    let widened = Signature::new(WEBGPU_F16, bits);
    assert_eq!(widened.deviations(), 4);
    assert_eq!(
        widened.traits(),
        "multiply_add=fused fma=fused swamped_sum=reassociated subnormal_product=flushed dot=reassociated"
    );
}

#[test]
fn the_combined_f16_set_is_read_as_the_f16_set() {
    let combined = correct(WEBGPU_F16_COMBINED);
    assert_eq!(combined.deviations(), 0);
    assert_eq!(combined.traits(), correct(WEBGPU_F16).traits());
}

#[test]
fn the_f16_answers_read_through_the_f32_interface_deviate() {
    // The same bits are wrong as f32 answers, which is why the interface names the set they answer.
    let misfiled = Signature::new(WEBGPU, f16_reference().into_iter().map(Answer::Bits).collect());
    assert_ne!(misfiled.deviations(), 0);
}
