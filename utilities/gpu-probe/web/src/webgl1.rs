//! The probe set as GLSL ES 1.00 fragment shaders, drawn with the page's WebGL 1.
//!
//! Some browsers offer WebGL 1 while withholding WebGL 2 and WebGPU -- the Tor Browser at its default security level
//! among them -- so this is the numeric signature such a page can take. GLSL ES 1.00 has no `floatBitsToUint`, so a
//! result is read back from a 32-bit float target where the browser allows one, and otherwise packed into the four
//! bytes of an 8-bit one by exact arithmetic in the shader. Either way the bits are the result's own, so the signature
//! is filed under the one interface. GLSL ES 1.00 has no `tanh` either, and the browser refuses that probe.
#![expect(
    clippy::single_call_fn,
    reason = "each step of opening WebGL 1 and measuring through it is a named function of its own"
)]

use et_gpu_probe::{Glsl, Signature, WEBGL1, WEBGL1_COMBINED, batched};
use js_sys::Float32Array;
use wasm_bindgen::JsCast as _;
use web_sys::WebGl2RenderingContext as Context;

use crate::batch::{self, layout};
use crate::targets::{attach, read, texture};
use crate::webgl2::{WebGl2Error, context_of, settled};

/// The fewest bits of mantissa a fragment shader's highp float may hold for a 32-bit result to be computed in it.
const F32_MANTISSA: i32 = 23;

/// What the page's WebGL 1 answers to the probe set, each probe in a shader of its own and then all in one.
///
/// Each is given by the interface it is filed under; where the browser gives no WebGL 1 fit to take one, why not.
pub fn signatures() -> Vec<(&'static str, Result<Signature, WebGl2Error>)> {
    let (context, glsl) = match context() {
        Ok(opened) => opened,
        Err(problem) => return vec![(WEBGL1, Err(problem))],
    };
    [("webgl1-split", false), (WEBGL1_COMBINED, true)]
        .into_iter()
        .map(|(interface, combined)| {
            let measured = batched(glsl, combined, |shader, width| {
                batch::draw(&context, glsl, shader, width)
            });
            (interface, measured)
        })
        .collect()
}

/// A WebGL 1 context with highp float in its fragment shaders, and the GLSL its results can be read back in.
///
/// The context is a WebGL 1 one, held as a WebGL 2 context for the calls the two share.
fn context() -> Result<(Context, Glsl), WebGl2Error> {
    let context: Context = context_of("webgl")?.ok_or(WebGl2Error::ContextV1)?.unchecked_into();
    let precision = context
        .get_shader_precision_format(Context::FRAGMENT_SHADER, Context::HIGH_FLOAT)
        .map_or(0_i32, |format| format.precision());
    if precision < F32_MANTISSA {
        return Err(WebGl2Error::Precision);
    }
    let glsl = if reads_floats(&context) {
        Glsl::Es100Float
    } else {
        Glsl::Es100Packed
    };
    Ok((context, glsl))
}

/// Whether `context` draws into a 32-bit float target and reads it back.
///
/// That takes `OES_texture_float` for the texture, and `WEBGL_color_buffer_float`, where the browser has it, to draw
/// into one; the Tor Browser allows both and refuses the read, which is why the read is tried too.
fn reads_floats(context: &Context) -> bool {
    let textures = context.get_extension("OES_texture_float").ok().flatten().is_some();
    let _drawable = context.get_extension("WEBGL_color_buffer_float");
    let name = "a float target";
    textures
        && texture(context, layout(Glsl::Es100Float), name, 1, None)
            .and_then(|target| attach(context, &target, name, 1))
            .and_then(|()| read(context, 1, Context::FLOAT, &Float32Array::new_with_length(4)))
            .and_then(|()| settled(context))
            .is_ok()
}
