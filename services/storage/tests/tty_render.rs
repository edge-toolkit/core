//! Exercise `tty_image::render_bytes`: the ANSI half-block render loop and its decode-error path.

#![cfg(test)]

use et_storage_service::tty_image::render_bytes;

#[test]
fn renders_a_small_image_to_ansi() {
    // Encode a tiny RGBA image in memory; the renderer decodes it back and walks the full
    // half-block loop (aspect-scaled resize, per-cell truecolor SGR emit, single stdout write).
    let img = image::RgbaImage::from_fn(3, 2, |_x, _y| image::Rgba([200, 100, 50, 255]));
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(img)
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    render_bytes(&png).unwrap();
}

#[test]
fn rejects_undecodable_bytes() {
    assert!(render_bytes(b"not an image").is_err());
}
