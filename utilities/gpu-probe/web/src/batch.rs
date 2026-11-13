//! A batch of the probe set drawn in the one fragment shader [`Glsl::fragment_shader`] writes, one pixel a probe.
//!
//! WebGL 1 and WebGL 2 draw it alike: into a new texture as wide as the batch, attached to a framebuffer, and read back
//! in one call. A WebGL 1 context is driven through the WebGL 2 bindings, which web-sys calls by the method's name on
//! the object, so a call the two share reaches the WebGL 1 context's own method.
#![expect(
    clippy::little_endian_bytes,
    reason = "an 8-bit pixel holds the result lowest byte first, as the shader packs it"
)]

use et_gpu_probe::{Batch, Glsl, INPUTS, TRIANGLE};
use js_sys::{Float32Array, Uint8Array, Uint32Array};
use web_sys::WebGl2RenderingContext as Context;

use crate::targets::{Layout, attach, read, texture};
use crate::webgl2::{WebGl2Error, link, settled};

/// What names the target in a refusal to draw into or read it.
const TARGET: &str = "the probes' target";

impl From<WebGl2Error> for Batch<WebGl2Error> {
    /// A shader the browser would not compile or link refuses the batch; anything else ends the measurement.
    fn from(problem: WebGl2Error) -> Self {
        match problem {
            WebGl2Error::Compile(log) | WebGl2Error::Link(log) => Self::Refused(log),
            other @ (WebGl2Error::Canvas
            | WebGl2Error::Context
            | WebGl2Error::ContextV1
            | WebGl2Error::Precision
            | WebGl2Error::Object(_)
            | WebGl2Error::Read(_)
            | WebGl2Error::Target(_)
            | WebGl2Error::Lost
            | WebGl2Error::Unsupported(_)
            | WebGl2Error::WebGl(_)) => Self::Failed(other),
        }
    }
}

/// How the target a batch in `glsl` draws into stores its pixels.
///
/// That is 8-bit RGBA, or 32-bit float RGBA where the result leaves the shader as a float.
pub fn layout(glsl: Glsl) -> Layout {
    // Glsl is non_exhaustive, so a match would need a wildcard that swallows the arms beside it.
    if glsl == Glsl::Es300 {
        [Context::RGBA8, Context::RGBA, Context::UNSIGNED_BYTE]
    } else if glsl == Glsl::Es100Float {
        [Context::RGBA, Context::RGBA, Context::FLOAT]
    } else {
        [Context::RGBA, Context::RGBA, Context::UNSIGNED_BYTE]
    }
}

/// Draw `fragment`, a batch of `width` probes in `glsl`, and read back the bits each pixel holds, in order.
///
/// # Errors
///
/// [`Batch::Refused`] when the browser will not compile or link the shader, and [`Batch::Failed`] when the target
/// cannot be drawn into or read, or the context reports an error or is lost.
pub fn draw(context: &Context, glsl: Glsl, fragment: &str, width: usize) -> Result<Vec<u32>, Batch<WebGl2Error>> {
    let program = link(context, glsl.vertex_shader(), fragment)?;
    let width = u32::try_from(width).unwrap_or(u32::MAX);
    let target = texture(context, layout(glsl), TARGET, width, None)?;
    attach(context, &target, TARGET, width)?;
    context.use_program(Some(&program));
    // GLSL ES 1.00 has no gl_VertexID, so its triangle's corners come from an attribute; GLSL ES 3.00 names none.
    if let Ok(corner) = u32::try_from(context.get_attrib_location(&program, "aPos")) {
        let corners = context.create_buffer().ok_or(WebGl2Error::Object("buffer"))?;
        context.bind_buffer(Context::ARRAY_BUFFER, Some(&corners));
        let triangle = Float32Array::from(TRIANGLE.as_slice());
        context.buffer_data_with_array_buffer_view(Context::ARRAY_BUFFER, &triangle, Context::STATIC_DRAW);
        context.enable_vertex_attrib_array(corner);
        context.vertex_attrib_pointer_with_i32(corner, 2, Context::FLOAT, false, 0, 0);
    }
    context.disable(Context::DITHER);
    context.disable(Context::BLEND);
    let inputs = context.get_uniform_location(&program, "probeIn");
    context.uniform1fv_with_f32_array(inputs.as_ref(), &INPUTS);
    context.clear_color(0.0, 0.0, 0.0, 0.0);
    context.clear(Context::COLOR_BUFFER_BIT);
    context.draw_arrays(Context::TRIANGLES, 0, 3);
    let values = width.saturating_mul(4);
    let bits = if glsl == Glsl::Es100Float {
        let floats = Float32Array::new_with_length(values);
        read(context, width, Context::FLOAT, &floats)?;
        // A float's bits are the result's own, the red of each pixel the first of its four.
        Uint32Array::new(&floats.buffer())
            .to_vec()
            .into_iter()
            .step_by(4)
            .collect()
    } else {
        let bytes = Uint8Array::new_with_length(values);
        read(context, width, Context::UNSIGNED_BYTE, &bytes)?;
        let bytes = bytes.to_vec();
        bytes
            .as_chunks::<4>()
            .0
            .iter()
            .copied()
            .map(u32::from_le_bytes)
            .collect()
    };
    settled(context)?;
    Ok(bits)
}
