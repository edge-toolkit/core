//! The two canvas fingerprints, drawn and read in the page the way the registry's canvas entries were taken.
//!
//! Each is taken more than once, because a browser defending against canvas fingerprinting answers with noise: a
//! fingerprint whose reads disagree is withheld, with the reason, rather than given as a signature.
#![expect(
    clippy::single_call_fn,
    reason = "each step of drawing and reading a canvas is a named function"
)]

use core::f64::consts::TAU;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use et_gpu_probe::{
    Alteration, CANVAS, CANVAS_BROWSERLEAKS, KNOWN_BLOCK, KNOWN_SIZE, alteration, known_colour, known_scene, pixel_hash,
};
use md5::{Digest as _, Md5};
use serde_json::{Value, json};
use wasm_bindgen::{JsCast as _, JsValue};
use web_sys::{CanvasRenderingContext2d, ContextAttributes2d, Document, HtmlCanvasElement, HtmlIFrameElement};

/// The width of the page's own scene.
const SCENE_WIDTH: u32 = 300;

/// The height of the page's own scene.
const SCENE_HEIGHT: u32 = 70;

/// Where the scene's two circles are centred, beside the colour each is drawn in.
///
/// The circles sit at 0.35 and 0.6 of the scene's width and half its height, with a radius of 0.4 of its height, and
/// the text at 0.55 and 0.85 of its height. Each of those products is exact in `f64`, so they are written out here
/// rather than computed, and the scene is drawn at the very coordinates the registry's entries were drawn at.
const CIRCLES: [(f64, &str); 2] = [(105.0, "#e33"), (180.0, "#39f")];

/// The height the scene's circles are centred at.
const CIRCLE_MIDDLE: f64 = 35.0;

/// The radius of the scene's circles.
const CIRCLE_RADIUS: f64 = 28.0;

/// The baseline of the scene's filled text.
const FILLED_BASELINE: f64 = 38.5;

/// The baseline of the scene's stroked text.
const STROKED_BASELINE: f64 = 59.5;

/// The text the canvas fingerprint browserleaks.com takes writes, twice.
const BROWSERLEAKS_TEXT: &str = "BrowserLeaks,com <canvas> 1.0";

/// The width of the canvas the fingerprint browserleaks.com takes draws on.
const BROWSERLEAKS_WIDTH: f64 = 220.0;

/// The height of the canvas the fingerprint browserleaks.com takes draws on.
const BROWSERLEAKS_HEIGHT: f64 = 30.0;

/// The read path the page's own scene is taken through.
const SCENE_PATH: Path = Path {
    frame: false,
    will_read_frequently: true,
    png: false,
};

/// The read path the canvas fingerprint browserleaks.com takes is taken through.
const BROWSERLEAKS_PATH: Path = Path {
    frame: true,
    will_read_frequently: false,
    png: true,
};

/// What stopped a canvas being drawn or read.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum CanvasError {
    /// The page has no document, or the sandboxed frame none of its own.
    #[error("the page has no document to make a canvas in")]
    Document,
    /// The browser made no object of the kind named.
    #[error("the browser made no {0}")]
    Object(&'static str),
    /// A canvas call threw, with what it threw.
    #[error("the canvas refused: {0}")]
    Refused(String),
    /// The PNG `toDataURL` gave was not a base64 data URL.
    #[error("toDataURL gave no base64 PNG")]
    DataUrl,
    /// The PNG `toDataURL` gave decodes to something other than 8-bit colour.
    #[error("toDataURL gave a PNG that does not decode to 8-bit colour")]
    Png,
    /// The data URL `toDataURL` gave did not hold valid base64.
    #[error("toDataURL gave a data URL that is not valid base64: {0}")]
    Base64(#[from] base64::DecodeError),
    /// The PNG `toDataURL` gave could not be decoded.
    #[error("toDataURL gave a PNG that could not be decoded: {0}")]
    Decode(#[from] png::DecodingError),
    /// A read of a canvas gave a different number of values from another read of it.
    #[error("two reads of one canvas disagree in length: {0}")]
    Length(#[from] et_gpu_probe::ReadLength),
}

impl From<JsValue> for CanvasError {
    fn from(thrown: JsValue) -> Self {
        Self::Refused(thrown.as_string().unwrap_or_else(|| format!("{thrown:?}")))
    }
}

/// A fingerprint every read agreed on, as the page shows the probe sets' signatures.
///
/// It is given by its `interface`, `results` as `[name, word]` pairs, and its `fingerprint`, `traits` and `describe`
/// lines.
fn signature(interface: &str, results: &[(String, String)], describe: &[String]) -> Value {
    let words: Vec<&str> = results.iter().map(|(_, word)| word.as_str()).collect();
    json!({
        "interface": interface,
        "results": results,
        "fingerprint": words.join("-"),
        "traits": "",
        "describe": describe,
    })
}

/// A fingerprint the browser answered with noise.
///
/// It is given by its `interface`, the `reason` it is withheld, and its `describe` lines.
fn withheld(interface: &str, reason: &str, describe: &[String]) -> Value {
    json!({ "interface": interface, "reason": reason, "describe": describe })
}

/// One way of making and reading a canvas.
#[derive(Clone, Copy, Eq, PartialEq)]
struct Path {
    /// Whether a sandboxed frame's document made the canvas, rather than the page's.
    frame: bool,
    /// Whether the canvas's context was asked for as one read back often.
    will_read_frequently: bool,
    /// Whether the canvas is read as the PNG `toDataURL` gives, rather than with `getImageData`.
    png: bool,
}

impl Path {
    /// Every way this page makes and reads a canvas.
    const ALL: [Self; 8] = [
        Self {
            frame: false,
            will_read_frequently: true,
            png: false,
        },
        Self {
            frame: false,
            will_read_frequently: true,
            png: true,
        },
        Self {
            frame: false,
            will_read_frequently: false,
            png: false,
        },
        Self {
            frame: false,
            will_read_frequently: false,
            png: true,
        },
        Self {
            frame: true,
            will_read_frequently: true,
            png: false,
        },
        Self {
            frame: true,
            will_read_frequently: true,
            png: true,
        },
        Self {
            frame: true,
            will_read_frequently: false,
            png: false,
        },
        Self {
            frame: true,
            will_read_frequently: false,
            png: true,
        },
    ];

    /// The path as a page shows it: where the canvas was made, its context, and how it was read.
    fn name(self) -> String {
        let home = if self.frame { "sandboxed frame" } else { "page" };
        let context = if self.will_read_frequently {
            "willReadFrequently"
        } else {
            "default"
        };
        let read = if self.png { "toDataURL" } else { "getImageData" };
        format!("{home}, {context}, {read}")
    }
}

/// The known scene read back through every way the page makes and reads a canvas.
///
/// A defence that adds noise to some of them alters the values read back on those, and not the others, which is seen
/// within a single visit: the known scene's every value is known before it is drawn.
pub struct ReadPaths {
    /// One line a path, saying how many values it altered, or why it could not be read.
    lines: Vec<String>,
    /// The paths that altered a value.
    altered: Vec<Path>,
}

impl ReadPaths {
    /// Whether `path` altered a value of the known scene.
    fn altered(&self, path: Path) -> bool {
        self.altered.contains(&path)
    }

    /// One line a path, saying how many values it altered, or why it could not be read.
    pub fn into_lines(self) -> Vec<String> {
        self.lines
    }
}

/// Read the known scene back through every [`Path`].
pub fn read_paths() -> Result<ReadPaths, CanvasError> {
    let document = document()?;
    let expected = known_scene();
    let mut paths = ReadPaths {
        lines: Vec::new(),
        altered: Vec::new(),
    };
    for path in Path::ALL {
        let read = if path.frame {
            in_frame(&document, |inner| read_known(inner, path))
        } else {
            read_known(&document, path)
        };
        let line = match read.map(|values| alteration(&values, &expected)) {
            Ok(Ok(found)) if found.changed == 0 => format!("{}: no value altered", path.name()),
            Ok(Ok(found)) => {
                paths.altered.push(path);
                format!(
                    "{}: {} of {} values altered, pattern {}",
                    path.name(),
                    found.changed,
                    found.total,
                    found.pattern
                )
            }
            Ok(Err(problem)) => format!("{}: {problem}", path.name()),
            Err(problem) => format!("{}: {problem}", path.name()),
        };
        paths.lines.push(line);
    }
    Ok(paths)
}

/// The known scene, drawn on a canvas `document` made and read back through `path`, as RGBA values.
fn read_known(document: &Document, path: Path) -> Result<Vec<u8>, CanvasError> {
    let canvas = canvas(document)?;
    canvas.set_width(u32::from(KNOWN_SIZE));
    canvas.set_height(u32::from(KNOWN_SIZE));
    let context = context_2d(&canvas, path.will_read_frequently)?;
    let block = f64::from(KNOWN_BLOCK);
    let step = usize::from(KNOWN_BLOCK);
    for (down, top) in (0_u8..).zip((0..KNOWN_SIZE).step_by(step)) {
        for (across, left) in (0_u8..).zip((0..KNOWN_SIZE).step_by(step)) {
            let [red, green, blue] = known_colour(across, down);
            context.set_fill_style_str(&format!("rgb({red},{green},{blue})"));
            context.fill_rect(f64::from(left), f64::from(top), block, block);
        }
    }
    if path.png {
        return decode_png(&png_bytes(&canvas)?);
    }
    let side = f64::from(KNOWN_SIZE);
    Ok(context.get_image_data(0.0, 0.0, side, side)?.data().0)
}

/// The RGBA values of `png`, an opaque PNG given back with or without its alpha channel.
fn decode_png(png: &[u8]) -> Result<Vec<u8>, CanvasError> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(png));
    decoder.set_transformations(png::Transformations::EXPAND);
    let mut reader = decoder.read_info()?;
    let mut buffer = vec![0; reader.output_buffer_size().ok_or(CanvasError::Png)?];
    let frame = reader.next_frame(&mut buffer)?;
    let bytes = buffer.get(..frame.buffer_size()).ok_or(CanvasError::Png)?;
    match frame.color_type {
        png::ColorType::Rgba => Ok(bytes.to_vec()),
        png::ColorType::Rgb => {
            let (pixels, _rest) = bytes.as_chunks::<3>();
            Ok(pixels
                .iter()
                .flat_map(|&[red, green, blue]| [red, green, blue, u8::MAX])
                .collect())
        }
        png::ColorType::Grayscale | png::ColorType::Indexed | png::ColorType::GrayscaleAlpha => Err(CanvasError::Png),
    }
}

/// The page's own scene, on a canvas the page made, hashed from `getImageData` twice.
///
/// It is withheld when the same read of the known scene in `paths` was altered.
pub fn scene(paths: &ReadPaths) -> Result<Value, CanvasError> {
    let document = document()?;
    let canvas = canvas(&document)?;
    canvas.set_width(SCENE_WIDTH);
    canvas.set_height(SCENE_HEIGHT);
    let context = context_2d(&canvas, true)?;
    let read = || -> Result<[String; 4], CanvasError> {
        draw_scene(&context, f64::from(SCENE_WIDTH), f64::from(SCENE_HEIGHT))?;
        let pixels = context.get_image_data(0.0, 0.0, f64::from(SCENE_WIDTH), f64::from(SCENE_HEIGHT))?;
        Ok(pixel_hash(&pixels.data()).map(|lane| format!("{lane:08x}")))
    };
    let first = read()?;
    let second = read()?;
    let describe = [
        format!("scene read 1: {}", first.join("-")),
        format!("scene read 2: {}", second.join("-")),
    ];
    if first != second {
        return Ok(withheld(
            CANVAS,
            "randomised: two reads of the same scene differed (per-read noise)",
            &describe,
        ));
    }
    if paths.altered(SCENE_PATH) {
        let reason = "randomised: getImageData altered a scene whose every value is known (noise added to reads)";
        return Ok(withheld(CANVAS, reason, &describe));
    }
    let results: Vec<_> = first
        .into_iter()
        .enumerate()
        .map(|(lane, word)| (format!("lane{lane}"), word))
        .collect();
    Ok(signature(CANVAS, &results, &describe))
}

/// The scene the registry's canvas entries hashed: a gradient, two circles multiplied over it, and two lines of text.
fn draw_scene(context: &CanvasRenderingContext2d, width: f64, height: f64) -> Result<(), CanvasError> {
    context.clear_rect(0.0, 0.0, width, height);
    let gradient = context.create_linear_gradient(0.0, 0.0, width, height);
    gradient.add_color_stop(0.0, "#f09")?;
    gradient.add_color_stop(0.5, "#0cf")?;
    gradient.add_color_stop(1.0, "#fc0")?;
    context.set_fill_style_canvas_gradient(&gradient);
    context.fill_rect(0.0, 0.0, width, height);
    context.set_global_composite_operation("multiply")?;
    for (across, colour) in CIRCLES {
        context.set_fill_style_str(colour);
        context.begin_path();
        context.arc(across, CIRCLE_MIDDLE, CIRCLE_RADIUS, 0.0, TAU)?;
        context.fill();
    }
    context.set_global_composite_operation("source-over")?;
    context.set_text_baseline("alphabetic");
    context.set_fill_style_str("#222");
    context.set_font("600 22px 'Times New Roman', Georgia, serif");
    context.fill_text("GPU fp \u{2728} 1234567890", 8.0, FILLED_BASELINE)?;
    context.set_stroke_style_str("rgba(0,0,0,0.5)");
    context.set_font("italic 16px system-ui, sans-serif");
    context.stroke_text("the quick brown fox", 10.0, STROKED_BASELINE)?;
    Ok(())
}

/// The canvas fingerprint browserleaks.com takes, in two sandboxed frames and on a canvas the page made.
///
/// The two frames stand for two loads of that site.
/// Its signature is the first frame's; a read in either frame that disagrees with it withholds the signature. The
/// page's own canvas is only shown, to say whether the frame changes the answer. It is withheld too when the same read
/// of the known scene in `paths` was altered.
pub fn browserleaks(paths: &ReadPaths) -> Result<Value, CanvasError> {
    let document = document()?;
    let first = in_frame(&document, browserleaks_reads)?;
    let second = in_frame(&document, browserleaks_reads)?;
    // The page's own canvas is only shown, so one the browser will not read leaves the frames' answer standing.
    let page = "canvas made by the page";
    let page_line = browserleaks_reads(&document).map_or_else(
        |problem| format!("{page}: not read, {problem}"),
        |reads| reads.line(page),
    );
    let describe = [first.line("frame 1"), second.line("frame 2"), page_line];
    if first.hash != first.again {
        let reason = "randomised: two reads of the same canvas differed (per-read noise)";
        return Ok(withheld(CANVAS_BROWSERLEAKS, reason, &describe));
    }
    if [&second.hash, &second.again].iter().any(|other| **other != first.hash) {
        let reason = "randomised: two sandboxed frames in one page gave different hashes";
        return Ok(withheld(CANVAS_BROWSERLEAKS, reason, &describe));
    }
    // Noise keyed to the session gives the same hash on every read of one visit, so only a read it leaves alone, or a
    // scene whose values are known, shows it.
    if first.against_pixels.changed != 0 {
        let reason = "randomised: toDataURL differed from getImageData of the same canvas (noise keyed to the session)";
        return Ok(withheld(CANVAS_BROWSERLEAKS, reason, &describe));
    }
    if paths.altered(BROWSERLEAKS_PATH) {
        let reason = "randomised: toDataURL altered a scene whose every value is known (noise keyed to the session)";
        return Ok(withheld(CANVAS_BROWSERLEAKS, reason, &describe));
    }
    Ok(signature(
        CANVAS_BROWSERLEAKS,
        &[("md5".to_owned(), first.hash)],
        &describe,
    ))
}

/// What `take` gives on the document of a fresh sandboxed frame in `document`, the frame removed after.
fn in_frame<Answer>(
    document: &Document,
    take: impl FnOnce(&Document) -> Result<Answer, CanvasError>,
) -> Result<Answer, CanvasError> {
    // An `iframe` element is an `HTMLIFrameElement`, so the cast needs no check.
    let frame = document.create_element("iframe")?.unchecked_into::<HtmlIFrameElement>();
    frame.set_attribute("sandbox", "allow-same-origin")?;
    frame.set_attribute("aria-hidden", "true")?;
    frame.set_attribute("style", "position:absolute;width:0;height:0;border:0")?;
    document
        .body()
        .ok_or(CanvasError::Document)?
        .append_with_node_1(&frame)?;
    let reads = frame
        .content_document()
        .ok_or(CanvasError::Document)
        .and_then(|inner| take(&inner));
    frame.remove();
    reads
}

/// What one canvas carrying the fingerprint's drawing gave back.
struct Reads {
    /// The MD5 of the PNG `toDataURL` gave.
    hash: String,
    /// The MD5 of the PNG a second `toDataURL` of the same canvas gave.
    again: String,
    /// How that PNG's values differ from those `getImageData` gives for the same canvas.
    against_pixels: Alteration,
    /// The [`pixel_hash`] of the values `getImageData` gives for the canvas.
    ///
    /// It says whether the image itself changed where the PNG's hash did.
    pixels: String,
}

impl Reads {
    /// The reads as a page shows them, opening with `place`.
    fn line(&self, place: &str) -> String {
        let Self {
            hash,
            again,
            against_pixels,
            pixels,
        } = self;
        let changed = against_pixels.changed;
        format!("{place}: {hash} then {again} on a second read; pixels {pixels}, the PNG differing in {changed} values")
    }
}

/// The fingerprint's drawing on a canvas `document` made, its PNG read twice and checked against its pixels.
///
/// A browser draws one canvas once, so its PNG and its `getImageData` hold the same values, and a defence that adds
/// noise to one read and not the other shows as a difference between them within a single visit.
fn browserleaks_reads(document: &Document) -> Result<Reads, CanvasError> {
    let canvas = canvas(document)?;
    let context = context_2d(&canvas, false)?;
    draw_browserleaks(&canvas, &context)?;
    let png = png_bytes(&canvas)?;
    let pixels = context
        .get_image_data(0.0, 0.0, BROWSERLEAKS_WIDTH, BROWSERLEAKS_HEIGHT)?
        .data()
        .0;
    let against_pixels = alteration(&decode_png(&png)?, &pixels)?;
    Ok(Reads {
        hash: md5_hex(&png),
        again: md5_hex(&png_bytes(&canvas)?),
        against_pixels,
        pixels: pixel_hash(&pixels).map(|lane| format!("{lane:08x}")).join("-"),
    })
}

/// The drawing the fingerprint browserleaks.com takes, call for call.
///
/// The canvas is sized after its context is taken, as that site does.
fn draw_browserleaks(canvas: &HtmlCanvasElement, context: &CanvasRenderingContext2d) -> Result<(), CanvasError> {
    canvas.set_attribute("width", &BROWSERLEAKS_WIDTH.to_string())?;
    canvas.set_attribute("height", &BROWSERLEAKS_HEIGHT.to_string())?;
    context.set_text_baseline("top");
    context.set_font("14px 'Arial'");
    context.set_text_baseline("alphabetic");
    context.set_fill_style_str("#f60");
    context.fill_rect(125.0, 1.0, 62.0, 20.0);
    context.set_fill_style_str("#069");
    context.fill_text(BROWSERLEAKS_TEXT, 2.0, 15.0)?;
    context.set_fill_style_str("rgba(102, 204, 0, 0.7)");
    context.fill_text(BROWSERLEAKS_TEXT, 4.0, 17.0)?;
    Ok(())
}

/// The MD5 of `bytes`, as 32 lowercase hex digits.
#[expect(
    clippy::big_endian_bytes,
    reason = "an MD5 digest is written out first byte first, which is big-endian"
)]
fn md5_hex(bytes: &[u8]) -> String {
    let digest: [u8; 16] = Md5::digest(bytes).into();
    format!("{:032x}", u128::from_be_bytes(digest))
}

/// The PNG `toDataURL` gives for `canvas`.
fn png_bytes(canvas: &HtmlCanvasElement) -> Result<Vec<u8>, CanvasError> {
    let url = canvas.to_data_url_with_type("image/png")?;
    let encoded = url.strip_prefix("data:image/png;base64,").ok_or(CanvasError::DataUrl)?;
    Ok(STANDARD.decode(encoded)?)
}

/// The page's document.
fn document() -> Result<Document, CanvasError> {
    web_sys::window()
        .and_then(|window| window.document())
        .ok_or(CanvasError::Document)
}

/// A canvas made by `document`, which the page does not show.
///
/// A frame's document makes its elements in the frame's own realm, where an `instanceof` check against the page's
/// `HTMLCanvasElement` fails, so the element is cast unchecked: a `canvas` element is an `HTMLCanvasElement` whichever
/// realm made it.
fn canvas(document: &Document) -> Result<HtmlCanvasElement, CanvasError> {
    Ok(document.create_element("canvas")?.unchecked_into::<HtmlCanvasElement>())
}

/// The 2-D context of `canvas`, cast unchecked for the reason [`canvas`] gives.
///
/// With `will_read_frequently` the context is asked for as one the page reads back often, which a browser may keep in
/// memory rather than on the GPU, and so draw and read differently.
fn context_2d(canvas: &HtmlCanvasElement, will_read_frequently: bool) -> Result<CanvasRenderingContext2d, CanvasError> {
    let context = if will_read_frequently {
        let attributes = ContextAttributes2d::new();
        attributes.set_will_read_frequently(true);
        canvas.get_context_with_context_options("2d", &attributes)?
    } else {
        canvas.get_context("2d")?
    };
    let context = context.ok_or(CanvasError::Object("2-D context"))?;
    Ok(context.unchecked_into::<CanvasRenderingContext2d>())
}
