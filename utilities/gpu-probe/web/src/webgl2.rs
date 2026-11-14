//! The probe set as GLSL ES 3.00 fragment shaders, each drawn to one pixel with the page's WebGL 2.
//!
//! A browser that gives no WebGPU adapter -- Chromium on Linux, by default -- still gives WebGL 2, so this is the
//! signature such a page can take. Each probe's shader writes its result's four bytes to the pixel's red, green, blue
//! and alpha, lowest first, so the pixel read back is the result in little-endian order.
#![expect(
    clippy::little_endian_bytes,
    clippy::single_call_fn,
    reason = "the pixel holds the result lowest byte first; each step of drawing a probe is a named function"
)]

use et_gpu_probe::{Answer, Glsl, INPUTS, Signature, VERTEX_SHADER, WEBGL2, batched, fragment_shaders};
use js_sys::Object;
use wasm_bindgen::{JsCast as _, JsValue};
use web_sys::{
    HtmlCanvasElement, WebGl2RenderingContext as Context, WebGlContextAttributes, WebGlProgram, WebGlShader,
};

use crate::batch;

/// What stopped the page drawing with WebGL 2, or one probe being drawn.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum WebGl2Error {
    /// The page has no document to make a canvas in.
    #[error("the page could not make a canvas")]
    Canvas,
    /// The browser gives no WebGL 2 context on the canvas.
    #[error("the browser gives no WebGL 2 context")]
    Context,
    /// The browser gives no WebGL 1 context on the canvas, so WebGL is turned off.
    #[error("the browser gives no WebGL 1 context")]
    ContextV1,
    /// The browser's WebGL 1 has no highp float in a fragment shader, in which no 32-bit result can be computed.
    #[error("the browser's WebGL 1 has no highp float in a fragment shader")]
    Precision,
    /// The browser made no object of the kind named.
    #[error("the browser made no {0}")]
    Object(&'static str),
    /// A shader did not compile, with the compiler's log.
    #[error("the shader did not compile: {0}")]
    Compile(String),
    /// The probe's shaders did not link, with the linker's log.
    #[error("the probe did not link: {0}")]
    Link(String),
    /// The pixel a probe drew could not be read back, with what the browser threw.
    #[error("the browser would not read the pixel back: {0}")]
    Read(String),
    /// A target of the format named cannot be drawn into, or read back exactly.
    #[error("the browser cannot draw into and read back {0} exactly")]
    Target(&'static str),
    /// The browser lost the WebGL context while the probe ran, so nothing it read back was drawn.
    #[error("the browser lost the WebGL context")]
    Lost,
    /// The probe named is of a kind this page has no WebGL 2 driver for.
    #[error("the page has no WebGL 2 driver for {0}")]
    Unsupported(&'static str),
    /// WebGL reported an error, by its code, while the probe ran.
    #[error("WebGL reported error {0:#06x}")]
    WebGl(u32),
}

impl From<JsValue> for WebGl2Error {
    /// What the browser threw from a read of the pixels a probe drew.
    fn from(thrown: JsValue) -> Self {
        Self::Read(thrown.as_string().unwrap_or_else(|| format!("{thrown:?}")))
    }
}

/// What the page's WebGL 2 answers to the probe set, each probe in a shader of its own.
///
/// A probe the browser will not compile or link is a refusal, given as the compiler's or linker's log.
pub fn signature() -> Result<Signature, WebGl2Error> {
    let context = context()?;
    let answers = fragment_shaders()
        .iter()
        .map(|(_, source)| {
            draw(&context, source).map_or_else(|refusal| Answer::Refused(refusal.to_string()), Answer::Bits)
        })
        .collect();
    Ok(Signature::new(WEBGL2, answers))
}

/// What the page's WebGL 2 answers to the probe set, every probe GLSL can express in one shader.
pub fn combined_signature() -> Result<Signature, WebGl2Error> {
    let context = context()?;
    batched(Glsl::Es300, true, |shader, width| {
        batch::draw(&context, Glsl::Es300, shader, width)
    })
}

/// A WebGL 2 context on a one-pixel canvas the page does not show.
pub(crate) fn context() -> Result<Context, WebGl2Error> {
    context_of("webgl2")?
        .and_then(|context| context.dyn_into::<Context>().ok())
        .ok_or(WebGl2Error::Context)
}

/// A context of the kind `kind` names, `webgl2` or `webgl`, on a one-pixel canvas, if the browser gives one.
///
/// The page does not show the canvas. Its drawing buffer is not antialiased and keeps alpha unpremultiplied, so the
/// pixel a probe draws reads back as exactly the bytes its shader wrote.
pub(crate) fn context_of(kind: &str) -> Result<Option<Object>, WebGl2Error> {
    let canvas = web_sys::window()
        .and_then(|window| window.document())
        .and_then(|document| document.create_element("canvas").ok())
        .and_then(|element| element.dyn_into::<HtmlCanvasElement>().ok())
        .ok_or(WebGl2Error::Canvas)?;
    canvas.set_width(1);
    canvas.set_height(1);
    let attributes = WebGlContextAttributes::new();
    attributes.set_antialias(false);
    attributes.set_premultiplied_alpha(false);
    attributes.set_preserve_drawing_buffer(true);
    Ok(canvas
        .get_context_with_context_options(kind, &attributes)
        .ok()
        .flatten())
}

/// Draw the probe whose fragment shader is `source` to the context's one pixel, and read back the bits it wrote.
fn draw(context: &Context, source: &str) -> Result<u32, WebGl2Error> {
    let program = link(context, VERTEX_SHADER, source)?;
    context.use_program(Some(&program));
    context.disable(Context::DITHER);
    let inputs = context.get_uniform_location(&program, "probeIn");
    context.uniform1fv_with_f32_array(inputs.as_ref(), &INPUTS);
    context.draw_arrays(Context::POINTS, 0, 1);
    let mut pixel = [0_u8; 4];
    let (rgba, bytes) = (Context::RGBA, Context::UNSIGNED_BYTE);
    context.read_pixels_with_opt_u8_array(0, 0, 1, 1, rgba, bytes, Some(&mut pixel))?;
    settled(context)?;
    Ok(u32::from_le_bytes(pixel))
}

/// Check the context drew what it was asked to: it is not lost, and reported no error while the probe ran.
///
/// A lost context, which WebGL allows at any moment, carries on taking calls and doing nothing, so a readback after it
/// gives the zeroes it started with rather than failing; its error clears once read, so the rest are read away too.
pub(crate) fn settled(context: &Context) -> Result<(), WebGl2Error> {
    if context.is_context_lost() {
        return Err(WebGl2Error::Lost);
    }
    let error = context.get_error();
    while context.get_error() != Context::NO_ERROR {}
    match error {
        Context::NO_ERROR => Ok(()),
        reported => Err(WebGl2Error::WebGl(reported)),
    }
}

/// The vertex shader `vertex` linked with the fragment shader `fragment`.
pub(crate) fn link(context: &Context, vertex: &str, fragment: &str) -> Result<WebGlProgram, WebGl2Error> {
    let vertex = compile(context, Context::VERTEX_SHADER, vertex)?;
    let fragment = compile(context, Context::FRAGMENT_SHADER, fragment)?;
    let program = context.create_program().ok_or(WebGl2Error::Object("program"))?;
    context.attach_shader(&program, &vertex);
    context.attach_shader(&program, &fragment);
    context.link_program(&program);
    if context.get_program_parameter(&program, Context::LINK_STATUS).as_bool() == Some(true) {
        return Ok(program);
    }
    Err(WebGl2Error::Link(
        context.get_program_info_log(&program).unwrap_or_default(),
    ))
}

/// `source` compiled as a shader of the kind `kind`.
fn compile(context: &Context, kind: u32, source: &str) -> Result<WebGlShader, WebGl2Error> {
    let shader = context.create_shader(kind).ok_or(WebGl2Error::Object("shader"))?;
    context.shader_source(&shader, source);
    context.compile_shader(&shader);
    if context.get_shader_parameter(&shader, Context::COMPILE_STATUS).as_bool() == Some(true) {
        return Ok(shader);
    }
    Err(WebGl2Error::Compile(
        context.get_shader_info_log(&shader).unwrap_or_default(),
    ))
}
