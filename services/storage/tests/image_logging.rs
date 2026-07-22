//! Tests for `is_image_filename`, the extension check behind the storage service's tty log line for
//! stored images (see `put_file` in `src/routes.rs`).

#![cfg(test)]

use std::path::Path;

use et_storage_service::routes::is_image_filename;

#[test]
fn recognizes_common_image_extensions_case_insensitively() {
    for ext in ["png", "jpg", "jpeg", "gif", "webp", "bmp", "PNG", "Jpg"] {
        let filename = format!("capture.{ext}");
        assert!(
            is_image_filename(Path::new(&filename)),
            "{filename} should be recognized as an image"
        );
    }
}

#[test]
fn rejects_non_image_extensions_and_extensionless_names() {
    for name in ["notes.txt", "data.json", "archive.tar.gz", "no-extension", ".hidden"] {
        assert!(
            !is_image_filename(Path::new(name)),
            "{name} should not be recognized as an image"
        );
    }
}
