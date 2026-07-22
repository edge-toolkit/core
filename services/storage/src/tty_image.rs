//! Hand-rolled ANSI truecolor half-block renderer for images on a real terminal.
//!
//! Kept dependency-free beyond the `image` crate (already resolved in the workspace via
//! deno_image/ws-web-runner, so this adds no new crate to review) rather than pulling in a terminal-image
//! crate: `viuer`, the obvious off-the-shelf choice, drags in `ansi_colours` (LGPL-3.0-or-later) as an
//! unconditional dependency for its RGB-to-256-color quantization path -- copyleft, and the first such
//! license this workspace would have had to accept. Emitting 24-bit truecolor SGR codes directly needs no
//! palette quantization at all, so there is nothing for that dependency to do here.
//!
//! Uses the "half-block" technique standard terminal-image tools (viu, chafa, catimg, viuer's own fallback)
//! use: each terminal row encodes two source-pixel rows via the upper-half-block character `\u{2580}`, whose
//! foreground and background colors are set independently, doubling the effective vertical resolution for
//! the same character-cell budget.

use std::fmt::Write as _;
use std::io::Write as _;
use std::path::Path;

/// Terminal columns the rendered thumbnail is resized to fit within.
const TARGET_COLUMNS: u32 = 48;

/// Render a thumbnail of the image at `path` directly to stdout using ANSI truecolor half-block art.
///
/// Returns the underlying `image` decode error on failure; the caller decides how to report it (this module
/// stays IO-boundary-agnostic rather than picking a logging mechanism itself).
#[expect(
    clippy::single_call_fn,
    reason = "distinct step of show_image_on_tty; kept separate for readability and testing"
)]
pub fn render(path: &Path) -> image::ImageResult<()> {
    let source = image::open(path)?;
    if source.width() == 0 || source.height() == 0 {
        return Ok(());
    }

    // Each output row consumes two source-pixel rows (the half-block trick above), so the resize target's
    // height is twice the column-derived row count; `max(2)` keeps a hairline-thin source image visible as
    // at least one row instead of rounding away to nothing. The float math and truncating cast are both
    // genuinely needed to turn a source aspect ratio into a terminal row count; TARGET_COLUMNS being a small
    // constant keeps the result comfortably within u32 for any real image.
    #[expect(
        clippy::float_arithmetic,
        reason = "aspect-ratio scaling to a row count needs float math"
    )]
    let aspect = f64::from(source.height()) / f64::from(source.width());
    #[expect(
        clippy::as_conversions,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::float_arithmetic,
        reason = "converting the aspect-scaled row count (always small and non-negative) back to u32"
    )]
    let rows = ((aspect * f64::from(TARGET_COLUMNS) / 2.0).round().max(1.0)) as u32;
    let sample_height = rows.saturating_mul(2).max(2);
    let resized = source
        .resize_exact(TARGET_COLUMNS, sample_height, image::imageops::FilterType::Triangle)
        .to_rgba8();

    let mut out = String::new();
    for row in 0..rows {
        let top_y = row.saturating_mul(2);
        let bottom_y = top_y.saturating_add(1);
        for col in 0..TARGET_COLUMNS {
            let top = resized.get_pixel(col, top_y);
            let bottom = resized.get_pixel(col, bottom_y);
            // Writing to a String cannot fail. A named (not bare `_`) binding discards the Result without
            // unwrap/expect or a bare `_`/`.ok()` (all denied by this workspace's lint set in different ways).
            let _write_result = write!(
                out,
                "\x1b[38;2;{};{};{}m\x1b[48;2;{};{};{}m\u{2580}",
                top[0], top[1], top[2], bottom[0], bottom[1], bottom[2]
            );
        }
        out.push_str("\x1b[0m\n");
    }

    // One write for the whole thumbnail (not one per escape code) keeps the render from interleaving with
    // other stdout lines on a shared terminal. A failed stdout write only costs tty visibility, not
    // correctness, so it's discarded here (named binding, not bare `_`/`.ok()`) rather than surfaced as
    // this function's own error.
    let _write_result = std::io::stdout().write_all(out.as_bytes());
    Ok(())
}
