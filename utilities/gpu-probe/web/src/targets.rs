//! The probe sets no shader compiler decides, put to the page's WebGL 2: clears, blends, filters and rasterization.
//!
//! Each probe draws into, or clears, a texture of its own attached to a framebuffer, and reads back the code its red
//! channel holds. A texel is read back in the type WebGL 2 reads its format in exactly: bytes for the 8-bit formats,
//! the packed word for the 10-bit one, and floats for the float formats, whose values each convert to f32 with no
//! rounding and so name the code they came from.
#![expect(
    clippy::single_call_fn,
    reason = "each step of drawing a probe is a named function of its own"
)]

use et_gpu_probe::{
    Answer, BLENDS, Blend, CLEARS, Clear, FILTERS, Filter, Format, Operation, RASTER_FORMAT, RASTER_WIDTH, RASTERS,
    Raster, Signature, WEBGL2_CLEAR, WEBGL2_FIXED_FUNCTION, coverage,
};
use js_sys::{Float32Array, Object, Uint8Array, Uint16Array, Uint32Array};
use web_sys::{WebGl2RenderingContext as Context, WebGlTexture};

use crate::webgl2::{WebGl2Error, context, link, settled};

/// The layout of the target a filter probe draws its sample into, which reads back as the sample's own bits.
const SAMPLE_LAYOUT: Layout = [Context::R32F, Context::RED, Context::FLOAT];

/// How a texture's texels are stored and handed over: its sized internal format, then its texels' format and type.
pub(crate) type Layout = [u32; 3];

/// What the page's WebGL 2 answers to the clears.
pub fn clear_signature() -> Result<Signature, WebGl2Error> {
    let context = float_context()?;
    let answers = CLEARS
        .iter()
        .map(|probe| answer(&context, || clear(&context, probe)))
        .collect();
    Ok(Signature::new(WEBGL2_CLEAR, answers))
}

/// What the page's WebGL 2 answers to the fixed-function probes: the blends, then the filters, then the rasters.
pub fn fixed_function_signature() -> Result<Signature, WebGl2Error> {
    let context = float_context()?;
    let blends = BLENDS.iter().map(|probe| answer(&context, || blend(&context, probe)));
    let filters = FILTERS.iter().map(|probe| answer(&context, || filter(&context, probe)));
    let rasters = RASTERS.iter().map(|probe| answer(&context, || raster(&context, probe)));
    let answers = blends.chain(filters).chain(rasters).collect();
    Ok(Signature::new(WEBGL2_FIXED_FUNCTION, answers))
}

/// A WebGL 2 context with float targets enabled where the browser offers them.
///
/// Without `EXT_color_buffer_float` a float texture cannot be attached to a framebuffer, so the probes that need one
/// are refused one by one rather than the whole set.
fn float_context() -> Result<Context, WebGl2Error> {
    let context = context()?;
    let _float_targets = context.get_extension("EXT_color_buffer_float");
    Ok(context)
}

/// What the device did with the probe `measure` runs: its code, or why it gave none.
///
/// A WebGL error the probe left behind is a refusal too, since a code read back after one means nothing.
fn answer(context: &Context, measure: impl FnOnce() -> Result<u32, WebGl2Error>) -> Answer {
    let outcome = measure().and_then(|code| settled(context).map(|()| code));
    // A probe refused partway may have left errors behind, which would otherwise be charged to the next one.
    let _cleared: Result<(), WebGl2Error> = settled(context);
    outcome.map_or_else(|refusal| Answer::Refused(refusal.to_string()), Answer::Bits)
}

/// Clear a one-texel target of the probe's format to its value, and read back the code it holds.
///
/// WebGL 2 takes a clear value as an f32, so the value is rounded to one before the driver converts it.
fn clear(context: &Context, probe: &Clear) -> Result<u32, WebGl2Error> {
    target(context, probe.format, 1, None)?;
    #[expect(
        clippy::as_conversions,
        clippy::cast_possible_truncation,
        reason = "clearBufferfv takes f32 values, and std has no other conversion from f64"
    )]
    let value = probe.value as f32;
    context.clear_bufferfv_with_f32_array(Context::COLOR, 0, &[value, 0.0, 0.0, 1.0]);
    read_red(context, probe.format)
}

/// Blend the probe's colour into a one-texel target holding its destination code, and read back the code it holds.
fn blend(context: &Context, probe: &Blend) -> Result<u32, WebGl2Error> {
    target(context, probe.format, 1, Some(&probe.format.texel(probe.destination)))?;
    let [vertex, fragment] = probe.glsl();
    context.use_program(Some(&link(context, &vertex, &fragment)?));
    let (source, destination) = match probe.operation {
        Operation::Multiply => (Context::DST_COLOR, Context::ZERO),
        Operation::Add => (Context::ONE, Context::ONE),
        _ => return Err(WebGl2Error::Unsupported(probe.name)),
    };
    context.enable(Context::BLEND);
    context.blend_equation(Context::FUNC_ADD);
    context.blend_func(source, destination);
    context.draw_arrays(Context::TRIANGLES, 0, 3);
    context.disable(Context::BLEND);
    read_red(context, probe.format)
}

/// Sample the probe's two-texel texture into a one-texel `r32float` target, and read back the sample's bits.
fn filter(context: &Context, probe: &Filter) -> Result<u32, WebGl2Error> {
    let sample = texture(context, SAMPLE_LAYOUT, "r32float", 1, None)?;
    attach(context, &sample, "r32float", 1)?;
    let [left, right] = probe.texels.map(|code| probe.format.texel(code));
    let name = probe.format.name();
    let _sampled: WebGlTexture = texture(context, layout(probe.format)?, name, 2, Some(&[left, right].concat()))?;
    for (parameter, value) in [
        (Context::TEXTURE_MIN_FILTER, Context::LINEAR),
        (Context::TEXTURE_MAG_FILTER, Context::LINEAR),
        (Context::TEXTURE_WRAP_S, Context::CLAMP_TO_EDGE),
        (Context::TEXTURE_WRAP_T, Context::CLAMP_TO_EDGE),
    ] {
        context.tex_parameteri(Context::TEXTURE_2D, parameter, gl_int(value));
    }
    let [vertex, fragment] = probe.glsl();
    let program = link(context, &vertex, &fragment)?;
    context.use_program(Some(&program));
    context.uniform1i(context.get_uniform_location(&program, "probeTexture").as_ref(), 0);
    context.draw_arrays(Context::TRIANGLES, 0, 3);
    let values = Float32Array::new_with_length(4);
    read(context, 1, Context::FLOAT, &values)?;
    Ok(values.get_index(0).to_bits())
}

/// Draw the probe's triangle across a target [`RASTER_WIDTH`] texels wide, and read back which texels it covered.
fn raster(context: &Context, probe: &Raster) -> Result<u32, WebGl2Error> {
    target(context, RASTER_FORMAT, RASTER_WIDTH, None)?;
    let [vertex, fragment] = probe.glsl();
    context.use_program(Some(&link(context, &vertex, &fragment)?));
    context.draw_arrays(Context::TRIANGLES, 0, 3);
    let texels = Uint8Array::new_with_length(RASTER_WIDTH.saturating_mul(RASTER_FORMAT.texel_bytes()));
    read(context, RASTER_WIDTH, Context::UNSIGNED_BYTE, &texels)?;
    Ok(coverage(&texels.to_vec()))
}

/// Draw into a new texture of `format`, `width` texels wide and one high, holding `texels` if given.
fn target(context: &Context, format: Format, width: u32, texels: Option<&[u8]>) -> Result<(), WebGl2Error> {
    let drawn = texture(context, layout(format)?, format.name(), width, texels)?;
    attach(context, &drawn, format.name(), width)
}

/// A new texture stored as `layout`, `width` texels wide and one high, holding `texels` if given, left bound.
///
/// The texels are laid out as [`Format::texel`] lays them, and handed over as the typed array the layout's type calls
/// for, which views the same bytes. `name` names the format in a refusal.
pub(crate) fn texture(
    context: &Context,
    layout: Layout,
    name: &'static str,
    width: u32,
    texels: Option<&[u8]>,
) -> Result<WebGlTexture, WebGl2Error> {
    let [internal, format, kind] = layout;
    let texture = context.create_texture().ok_or(WebGl2Error::Object("texture"))?;
    context.bind_texture(Context::TEXTURE_2D, Some(&texture));
    let data = texels.map(|bytes| typed(bytes, kind));
    let (target, level, border) = (Context::TEXTURE_2D, 0_i32, 0_i32);
    let uploaded = context.tex_image_2d_with_i32_and_i32_and_i32_and_format_and_type_and_opt_array_buffer_view(
        target,
        level,
        gl_int(internal),
        gl_int(width),
        1,
        border,
        format,
        kind,
        data.as_ref(),
    );
    if uploaded.is_err() {
        return Err(WebGl2Error::Target(name));
    }
    Ok(texture)
}

/// Attach `texture` to a new framebuffer, and draw into its `width` texels from then on.
pub(crate) fn attach(
    context: &Context,
    texture: &WebGlTexture,
    name: &'static str,
    width: u32,
) -> Result<(), WebGl2Error> {
    let framebuffer = context.create_framebuffer().ok_or(WebGl2Error::Object("framebuffer"))?;
    context.bind_framebuffer(Context::FRAMEBUFFER, Some(&framebuffer));
    let (attachment, level) = (Context::COLOR_ATTACHMENT0, 0_i32);
    context.framebuffer_texture_2d(
        Context::FRAMEBUFFER,
        attachment,
        Context::TEXTURE_2D,
        Some(texture),
        level,
    );
    if context.check_framebuffer_status(Context::FRAMEBUFFER) != Context::FRAMEBUFFER_COMPLETE {
        return Err(WebGl2Error::Target(name));
    }
    context.viewport(0, 0, gl_int(width), 1);
    Ok(())
}

/// How WebGL 2 stores a texture of `format`, and the format and type a texel of it uploads as.
const fn layout(format: Format) -> Result<Layout, WebGl2Error> {
    Ok(match format {
        Format::Rgba8Unorm => [Context::RGBA8, Context::RGBA, Context::UNSIGNED_BYTE],
        Format::Rgba8UnormSrgb => [Context::SRGB8_ALPHA8, Context::RGBA, Context::UNSIGNED_BYTE],
        Format::Rgb10a2Unorm => [Context::RGB10_A2, Context::RGBA, Context::UNSIGNED_INT_2_10_10_10_REV],
        Format::Rg11b10Ufloat => [
            Context::R11F_G11F_B10F,
            Context::RGB,
            Context::UNSIGNED_INT_10F_11F_11F_REV,
        ],
        Format::Rgba16Float => [Context::RGBA16F, Context::RGBA, Context::HALF_FLOAT],
        other => return Err(WebGl2Error::Target(other.name())),
    })
}

/// `bytes` as the typed array WebGL 2 takes texels of the type `kind` in, viewing the same memory.
fn typed(bytes: &[u8], kind: u32) -> Object {
    let buffer = Uint8Array::from(bytes).buffer();
    match kind {
        Context::UNSIGNED_BYTE => Uint8Array::new(&buffer).into(),
        Context::HALF_FLOAT => Uint16Array::new(&buffer).into(),
        _ => Uint32Array::new(&buffer).into(),
    }
}

/// The code the red channel of the target's first texel holds, the target being of `format`.
fn read_red(context: &Context, format: Format) -> Result<u32, WebGl2Error> {
    match format {
        Format::Rgba8Unorm | Format::Rgba8UnormSrgb => {
            let texel = Uint8Array::new_with_length(4);
            read(context, 1, Context::UNSIGNED_BYTE, &texel)?;
            Ok(u32::from(texel.get_index(0)))
        }
        Format::Rgb10a2Unorm => {
            let word = Uint32Array::new_with_length(1);
            read(context, 1, Context::UNSIGNED_INT_2_10_10_10_REV, &word)?;
            #[expect(
                clippy::little_endian_bytes,
                reason = "Format::red reads a texel lowest bits first, as WebGPU lays it out and this word packs it"
            )]
            let texel = word.get_index(0).to_le_bytes();
            Ok(format.red(texel))
        }
        Format::Rg11b10Ufloat | Format::Rgba16Float => {
            let texel = Float32Array::new_with_length(4);
            read(context, 1, Context::FLOAT, &texel)?;
            Ok(format.encode(f64::from(texel.get_index(0))))
        }
        other => Err(WebGl2Error::Target(other.name())),
    }
}

/// Read the first `width` texels of the target as RGBA of the type `kind` into `into`.
pub(crate) fn read(context: &Context, width: u32, kind: u32, into: &Object) -> Result<(), WebGl2Error> {
    context.read_pixels_with_opt_array_buffer_view(0, 0, gl_int(width), 1, Context::RGBA, kind, Some(into))?;
    Ok(())
}

/// `value`, a WebGL enum or a size of a few texels, as the `GLint` WebGL takes it in.
fn gl_int(value: u32) -> i32 {
    i32::try_from(value).unwrap_or(i32::MAX)
}
