//! Probes of the GPU's fixed-function stages: blending, texture filtering and rasterization.
//!
//! Each still draws with a shader, but one that only hands the stage a constant or a coordinate: the arithmetic whose
//! result is read back is done by the blend unit, the texture unit or the rasterizer, not by anything a shader compiler
//! emits. Each draws one triangle into a target one texel high and reads the target back.

use crate::Format;

/// How many fixed-function probes the set holds.
pub const FIXED_FUNCTION_COUNT: usize = 9;

/// How many texels wide the target a raster probe draws into is.
pub const RASTER_WIDTH: u32 = 8;

/// The format of the target a raster probe draws into, whose red channel is set wherever the triangle covers.
pub const RASTER_FORMAT: Format = Format::Rgba8Unorm;

/// Every blend probe, in output order.
///
/// The first multiplies 101 by one half, a tie between two 8-bit codes. The second adds to one a value three quarters
/// of the way to the next half-precision float. The third adds to an sRGB code, which the blend has to decode and
/// encode again.
pub const BLENDS: [Blend; 3] = [
    Blend {
        name: "blend_multiply",
        format: Format::Rgba8Unorm,
        destination: 101,
        source: 0.5,
        operation: Operation::Multiply,
    },
    Blend {
        name: "blend_half_float",
        format: Format::Rgba16Float,
        destination: 0x3c00,
        source: 1.0 / 2048.0 + 1.0 / 8192.0,
        operation: Operation::Add,
    },
    Blend {
        name: "blend_srgb",
        format: Format::Rgba8UnormSrgb,
        destination: 188,
        source: 0.25,
        operation: Operation::Add,
    },
];

/// Every filter probe, in output order.
///
/// The first two sample between black and white a third and a thousandth of the way: a texture unit keeps only so many
/// bits of the weight, and the thousandth is below what 8 bits hold. The third samples halfway between two sRGB codes,
/// which is a different value when the texels are filtered before they are decoded rather than after. Each coordinate
/// is midway between 0.5 and the weight, since the two texel centres sit at a quarter and three quarters.
pub const FILTERS: [Filter; 3] = [
    Filter {
        name: "filter_third",
        format: Format::Rgba8Unorm,
        texels: [0, 255],
        coordinate: f32::midpoint(0.5, 1.0 / 3.0),
    },
    Filter {
        name: "filter_fine",
        format: Format::Rgba8Unorm,
        texels: [0, 255],
        coordinate: f32::midpoint(0.5, 1.0 / 1000.0),
    },
    Filter {
        name: "filter_srgb",
        format: Format::Rgba8UnormSrgb,
        texels: [0, 128],
        coordinate: 0.5,
    },
];

/// Every raster probe, in output order.
///
/// Each puts its edge a little right of the centre of texel 3, at 3.5: by 2^-10 of a texel, which a rasterizer that
/// snaps vertices to 8 fractional bits moves onto the centre, and by 2^-6, which one with only 4 bits does. A centre on
/// the edge is covered when it is the triangle's left edge and not when it is its right, so snapping shows.
pub const RASTERS: [Raster; 3] = [
    Raster {
        name: "raster_right_edge_fine",
        edge: 3.5 + 1.0 / 1024.0,
        side: Side::Left,
    },
    Raster {
        name: "raster_left_edge_fine",
        edge: 3.5 + 1.0 / 1024.0,
        side: Side::Right,
    },
    Raster {
        name: "raster_right_edge_coarse",
        edge: 3.5 + 1.0 / 64.0,
        side: Side::Left,
    },
];

/// For each fixed-function probe with more than one answer worth naming: its name, then each answer beside its meaning.
pub(crate) const CHOICES: [(&str, &[(u32, &str)]); 4] = [
    ("blend_half_float", &[(0x3c01, "rounded"), (0x3c00, "truncated")]),
    ("raster_right_edge_fine", &[(0x0f, "exact"), (0x07, "snapped")]),
    ("raster_left_edge_fine", &[(0xf0, "exact"), (0xf8, "snapped")]),
    ("raster_right_edge_coarse", &[(0x0f, "exact"), (0x07, "snapped")]),
];

/// The vertices of a triangle covering the whole of any target, in clip space.
const FULL_SCREEN: [[f32; 2]; 3] = [[-1.0, -1.0], [3.0, -1.0], [-1.0, 3.0]];

/// What a blend probe's blend unit does with the colour the shader gives and the colour the target holds.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Operation {
    /// The product of the two: source factor the destination colour, destination factor zero, added.
    Multiply,
    /// The sum of the two: both factors one, added.
    Add,
}

/// A blend probe: a constant colour blended into a one-texel target that holds a code to start with.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct Blend {
    /// The probe's name.
    pub name: &'static str,
    /// The format of the target.
    pub format: Format,
    /// The code the target's red channel holds before the draw, uploaded as a texel of [`Format::texel`].
    pub destination: u32,
    /// The value the shader gives every channel of the colour it blends in.
    pub source: f32,
    /// What the blend unit does with the two.
    pub operation: Operation,
}

/// A filter probe: a two-texel texture sampled with linear filtering between its texels, at one coordinate.
///
/// The texture is two texels wide and one high, and is sampled at level 0 with a linear minification and magnification
/// filter and clamp-to-edge addressing, at ([`Filter::coordinate`], 0.5). Its red channels hold [`Filter::texels`]. The
/// sample's red channel is drawn into a one-texel `r32float` target and read back as bits.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct Filter {
    /// The probe's name.
    pub name: &'static str,
    /// The format of the sampled texture.
    pub format: Format,
    /// The codes the texture's two texels hold in their red channels, left first.
    pub texels: [u32; 2],
    /// The horizontal coordinate sampled at, from 0 to 1 across the texture.
    pub coordinate: f32,
}

/// Which side of its vertical edge a raster probe's triangle lies on.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Side {
    /// The triangle lies left of the edge, which is then its right edge.
    Left,
    /// The triangle lies right of the edge, which is then its left edge.
    Right,
}

/// A raster probe: a triangle with one vertical edge, drawn across a target [`RASTER_WIDTH`] texels wide.
///
/// The triangle covers the whole height of the target, so which texels it covers depends only on where its edge falls
/// against their centres, and is read back as a mask: bit `i` set when texel `i` was covered.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct Raster {
    /// The probe's name.
    pub name: &'static str,
    /// Where the vertical edge falls, in texels from the target's left side.
    pub edge: f32,
    /// Which side of the edge the triangle lies on.
    pub side: Side,
}

/// Every fixed-function probe's name, in output order: the blends, then the filters, then the rasters.
#[expect(
    clippy::single_call_fn,
    reason = "kept beside the three lists it reads, which the probe set is built from elsewhere"
)]
pub(crate) fn names() -> Vec<&'static str> {
    BLENDS
        .iter()
        .map(|blend| blend.name)
        .chain(FILTERS.iter().map(|filter| filter.name))
        .chain(RASTERS.iter().map(|raster| raster.name))
        .collect()
}

impl Blend {
    /// The WGSL that draws the probe: a triangle over the whole target, in [`Blend::source`] on every channel.
    ///
    /// Its entry points are `vertex` and `fragment`, and it binds nothing.
    #[must_use]
    pub fn shader(&self) -> String {
        let source = literal(self.source);
        let colour = format!("vec4<f32>({source}, {source}, {source}, {source})");
        render_shader(FULL_SCREEN, "", &colour)
    }

    /// The GLSL ES 3.00 vertex and fragment shaders that draw the probe on WebGL 2, as [`Blend::shader`] draws it.
    ///
    /// They bind nothing, and draw three vertices as triangles with no attributes.
    #[must_use]
    pub fn glsl(&self) -> [String; 2] {
        let source = glsl_literal(self.source);
        glsl_program(FULL_SCREEN, "", &format!("vec4({source})"))
    }

    /// The code the target's red channel holds after the draw, computed exactly and rounded once.
    #[must_use]
    #[expect(clippy::float_arithmetic, reason = "a blend is float arithmetic on the two colours")]
    pub fn reference(&self) -> u32 {
        let destination = self.format.decode(self.destination);
        let source = f64::from(self.source);
        self.format.encode(match self.operation {
            Operation::Multiply => source * destination,
            Operation::Add => source + destination,
        })
    }
}

impl Filter {
    /// The WGSL that draws the probe: a triangle over the one-texel target, in the sample's red channel.
    ///
    /// Its entry points are `vertex` and `fragment`, and it binds the texture at binding 0 and the sampler at 1.
    #[must_use]
    pub fn shader(&self) -> String {
        let bindings = concat!(
            "@group(0) @binding(0) var probeTexture : texture_2d<f32>;\n",
            "@group(0) @binding(1) var probeSampler : sampler;\n"
        );
        let point = format!("vec2<f32>({}, 0.5)", literal(self.coordinate));
        let sample = format!("textureSampleLevel(probeTexture, probeSampler, {point}, 0.0).r");
        let colour = format!("vec4<f32>({sample}, 0.0, 0.0, 1.0)");
        render_shader(FULL_SCREEN, bindings, &colour)
    }

    /// The GLSL ES 3.00 vertex and fragment shaders that draw the probe on WebGL 2, as [`Filter::shader`] draws it.
    ///
    /// The fragment shader samples the texture bound to its `probeTexture` uniform, whose own parameters give the
    /// filtering and addressing the probe calls for.
    #[must_use]
    pub fn glsl(&self) -> [String; 2] {
        let point = format!("vec2({}, 0.5)", glsl_literal(self.coordinate));
        let colour = format!("vec4(textureLod(probeTexture, {point}, 0.0).r, 0.0, 0.0, 1.0)");
        glsl_program(FULL_SCREEN, "uniform highp sampler2D probeTexture;\n", &colour)
    }

    /// The bits of the f32 the sample's red channel is, filtered exactly between the decoded texels.
    #[must_use]
    #[expect(
        clippy::float_arithmetic,
        reason = "linear filtering is float arithmetic on the two texels"
    )]
    pub fn reference(&self) -> u32 {
        let [left, right] = self.texels.map(|code| self.format.decode(code));
        let weight = f64::from(self.coordinate).mul_add(2.0_f64, -0.5_f64);
        crate::nearest((right - left).mul_add(weight, left)).to_bits()
    }
}

impl Raster {
    /// The WGSL that draws the probe: its triangle, in white.
    ///
    /// Its entry points are `vertex` and `fragment`, and it binds nothing.
    #[must_use]
    pub fn shader(&self) -> String {
        render_shader(self.corners(), "", "vec4<f32>(1.0)")
    }

    /// The GLSL ES 3.00 vertex and fragment shaders that draw the probe on WebGL 2, as [`Raster::shader`] draws it.
    ///
    /// They bind nothing, and draw three vertices as triangles with no attributes.
    #[must_use]
    pub fn glsl(&self) -> [String; 2] {
        glsl_program(self.corners(), "", "vec4(1.0)")
    }

    /// The triangle's corners in clip space: two on the edge, above and below the target, and one far to its side.
    #[expect(
        clippy::float_arithmetic,
        reason = "the edge's clip-space position is its texel position scaled by a power of two, and exact"
    )]
    fn corners(&self) -> [[f32; 2]; 3] {
        let across = self.edge / 4.0 - 1.0;
        let far = match self.side {
            Side::Left => -5.0,
            Side::Right => 5.0,
        };
        [[across, -3.0], [across, 3.0], [far, 0.0]]
    }

    /// The mask of the texels the triangle covers, with no snapping.
    ///
    /// A centre on the edge is counted as a rasterizer counts it: covered for a left edge, not for a right one.
    #[must_use]
    #[expect(
        clippy::float_arithmetic,
        reason = "a texel's centre is half a texel past its left side"
    )]
    pub fn reference(&self) -> u32 {
        (0..RASTER_WIDTH)
            .filter(|texel| {
                let centre = f64::from(*texel) + 0.5_f64;
                let edge = f64::from(self.edge);
                match self.side {
                    Side::Left => centre < edge,
                    Side::Right => centre >= edge,
                }
            })
            .fold(0, |mask, texel| mask | (1_u32 << texel))
    }
}

/// The mask of the texels of a raster probe's target that the triangle covered, from the target's bytes.
///
/// Bit `i` is set when the red channel of texel `i` is not zero.
#[must_use]
pub fn coverage(texels: &[u8]) -> u32 {
    let texel_bytes = usize::try_from(RASTER_FORMAT.texel_bytes()).unwrap_or(usize::MAX);
    texels
        .chunks(texel_bytes)
        .zip(0..RASTER_WIDTH)
        .filter(|(texel, _)| texel.first().is_some_and(|red| *red != 0))
        .fold(0, |mask, (_, index)| mask | (1_u32 << index))
}

/// The answer each fixed-function probe gives when computed exactly, in output order.
#[must_use]
pub fn fixed_function_reference() -> Vec<u32> {
    BLENDS
        .iter()
        .map(Blend::reference)
        .chain(FILTERS.iter().map(Filter::reference))
        .chain(RASTERS.iter().map(Raster::reference))
        .collect()
}

/// `value` as a WGSL expression for exactly that f32, which a decimal literal can only round to.
fn literal(value: f32) -> String {
    format!("bitcast<f32>({:#010x}u)", value.to_bits())
}

/// A WGSL vertex and fragment shader drawing the triangle `corners`, in clip space, in the colour `colour`.
///
/// `bindings` declares whatever `colour` reads. The entry points are `vertex` and `fragment`.
fn render_shader(corners: [[f32; 2]; 3], bindings: &str, colour: &str) -> String {
    let corners = corner_list(corners, "vec2<f32>", literal);
    format!(
        "{bindings}
@vertex
fn vertex(@builtin(vertex_index) index : u32) -> @builtin(position) vec4<f32> {{
    var corners = array<vec2<f32>, 3>({corners});
    return vec4<f32>(corners[index], 0.0, 1.0);
}}

@fragment
fn fragment() -> @location(0) vec4<f32> {{
    return {colour};
}}
"
    )
}

/// `value` as a GLSL ES 3.00 expression for exactly that f32, which a decimal literal can only round to.
fn glsl_literal(value: f32) -> String {
    format!("uintBitsToFloat({:#010x}u)", value.to_bits())
}

/// GLSL ES 3.00 vertex and fragment shaders drawing the triangle `corners`, in clip space, in the colour `colour`.
///
/// `declarations` declares whatever `colour` reads. The vertex shader takes each corner from `gl_VertexID`.
fn glsl_program(corners: [[f32; 2]; 3], declarations: &str, colour: &str) -> [String; 2] {
    let corners = corner_list(corners, "vec2", glsl_literal);
    let vertex = format!(
        "#version 300 es
void main() {{
    vec2 corners[3] = vec2[3]({corners});
    gl_Position = vec4(corners[gl_VertexID], 0.0, 1.0);
}}
"
    );
    let fragment = format!(
        "#version 300 es
precision highp float;
{declarations}out highp vec4 probeOut;
void main() {{
    probeOut = {colour};
}}
"
    );
    [vertex, fragment]
}

/// `corners` as a comma-separated list of `constructor` calls, each coordinate written exactly by `literal`.
fn corner_list(corners: [[f32; 2]; 3], constructor: &str, literal: fn(f32) -> String) -> String {
    corners
        .iter()
        .map(|[across, upward]| format!("{constructor}({}, {})", literal(*across), literal(*upward)))
        .collect::<Vec<_>>()
        .join(", ")
}
