//! Symlink-aware module discovery + serving.
//!
//! mise's embedded aube npm backend reaches a package through *two* symlink
//! hops: `node_modules/<pkg>` points at
//! `node_modules/.mise/<pkg>@<version>/node_modules/<pkg>`, and that
//! `.mise/<pkg>@<version>` entry points in turn at the content-addressed aube
//! virtual store. The modules service has to follow the whole chain both when
//! scanning (`list_modules`) and when actix-files serves files out of the
//! discovered package dir. The fixture below mirrors both hops so a regression
//! in multi-hop resolution is caught, not just single-hop. Regressing either
//! half manifests as a 404 on `/modules/onnxruntime-web/dist/ort.min.js` (and
//! similar) which is what these tests pin down.

#![cfg(test)]
#![cfg(unix)]
#![expect(
    clippy::deref_by_slicing,
    reason = "test code: slice-deref in fixture assertions is intentional"
)]

use std::os::unix::fs::symlink;
use std::path::PathBuf;

use actix_web::http::StatusCode;
use actix_web::{App, test, web};
use edge_toolkit::ws_server::AgentRegistry;
use et_modules_service::{ModulesConfig, configure, list_modules};
use fs_err as fs;
use tempfile::TempDir;

const ORT_BUNDLE: &[u8] = b"// pretend ort.min.js bundle";

/// Build a fixture mirroring mise's two-hop npm layout.
///
/// `<store>/<pkg>@<version>-<hash>/` holds the real package, standing in for the aube virtual store.
/// `<scan>` plays the install's `node_modules` and is what we hand to `ModulesConfig::paths`; within it
/// `<pkg>` symlinks to `.mise/<pkg>@<version>/node_modules/<pkg>`, and `.mise/<pkg>@<version>` symlinks to
/// the store entry -- so resolving a package traverses both hops exactly as it does against a real install.
/// Returns both tempdirs (kept alive by the caller) plus the modules config.
fn mise_layout_fixture() -> (TempDir, TempDir, ModulesConfig) {
    let store = TempDir::new().unwrap();
    let scan = TempDir::new().unwrap();
    let farm = scan.path().join(".mise");
    fs::create_dir_all(&farm).unwrap();

    // Second hop target: the store entry, named as aube names its virtual-store dirs.
    let real_pkg = store.path().join("onnxruntime-web@1.27.0-ac0bad64e3fabd3b");
    fs::create_dir_all(real_pkg.join("node_modules/onnxruntime-web/dist")).unwrap();
    let pkg_inner = real_pkg.join("node_modules/onnxruntime-web");
    fs::write(
        pkg_inner.join("package.json"),
        r#"{"name":"onnxruntime-web","version":"1.27.0"}"#,
    )
    .unwrap();
    fs::write(pkg_inner.join("dist/ort.min.js"), ORT_BUNDLE).unwrap();

    // Also drop an et-ws-server-static stub -- `configure` panics if the
    // configured `root` module can't be found, so we satisfy that here too.
    let static_store = store.path().join("et-ws-server-static@0.0.0-0000000000000000");
    let static_inner = static_store.join("node_modules/et-ws-server-static");
    fs::create_dir_all(&static_inner).unwrap();
    fs::write(
        static_inner.join("package.json"),
        r#"{"name":"et-ws-server-static","version":"0.0.0"}"#,
    )
    .unwrap();
    fs::write(static_inner.join("index.html"), b"<!doctype html>").unwrap();

    // Hop 2: `.mise/<pkg>@<version>` -> the store entry.
    symlink(&real_pkg, farm.join("onnxruntime-web@1.27.0")).unwrap();
    symlink(&static_store, farm.join("et-ws-server-static@0.0.0")).unwrap();

    // Hop 1: `<pkg>` -> `.mise/<pkg>@<version>/node_modules/<pkg>`, relative just as mise writes it.
    symlink(
        PathBuf::from(".mise/onnxruntime-web@1.27.0/node_modules/onnxruntime-web"),
        scan.path().join("onnxruntime-web"),
    )
    .unwrap();
    symlink(
        PathBuf::from(".mise/et-ws-server-static@0.0.0/node_modules/et-ws-server-static"),
        scan.path().join("et-ws-server-static"),
    )
    .unwrap();

    let config = ModulesConfig::new(vec![scan.path().to_path_buf()], "et-ws-server-static".to_string());
    (store, scan, config)
}

// `actix_rt::test` because the file imports `actix_web::test` at module
// scope, which shadows the built-in `#[test]` attribute. The body is
// synchronous; the async wrapper is harmless.
#[actix_rt::test]
async fn list_modules_follows_symlinks_to_package_dirs() {
    let (_store, _scan, config) = mise_layout_fixture();

    let found: Vec<(String, PathBuf)> = list_modules(&config);

    // Both packages -- the symlinked target onnxruntime-web and the
    // symlinked stub root module -- should be discovered.
    let by_name: std::collections::HashMap<&str, &PathBuf> =
        found.iter().map(|(name, path)| (name.as_str(), path)).collect();
    let pkg_path = &by_name["onnxruntime-web"];

    // The discovered path must resolve to the real package dir so that
    // `Files::new("/modules/onnxruntime-web", pkg_path)` can serve
    // `dist/ort.min.js`.
    assert!(
        pkg_path.join("dist/ort.min.js").exists(),
        "expected dist/ort.min.js reachable through the symlink, got pkg_path = {pkg_path:?}",
    );
}

#[actix_rt::test]
async fn lists_symlinked_module_in_modules_api() {
    let (_store, _scan, config) = mise_layout_fixture();
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(AgentRegistry::<()>::default()))
            .app_data(web::Data::new(config.clone()))
            .configure(|cfg| configure(cfg, &config)),
    )
    .await;

    let req = test::TestRequest::get().uri("/modules/").to_request();
    let names: Vec<String> = test::call_and_read_body_json(&app, req).await;

    assert!(
        names.contains(&"onnxruntime-web".to_string()),
        "expected `onnxruntime-web` in the listing, got {names:?}",
    );
}

#[actix_rt::test]
async fn serves_file_under_symlinked_module() {
    let (_store, _scan, config) = mise_layout_fixture();
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(AgentRegistry::<()>::default()))
            .app_data(web::Data::new(config.clone()))
            .configure(|cfg| configure(cfg, &config)),
    )
    .await;

    let req = test::TestRequest::get()
        .uri("/modules/onnxruntime-web/dist/ort.min.js")
        .to_request();
    let resp = test::call_service(&app, req).await;

    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "expected 200 from /modules/onnxruntime-web/dist/ort.min.js, got {}",
        resp.status(),
    );
    let body = test::read_body(resp).await;
    assert_eq!(&body[..], ORT_BUNDLE);
}

#[actix_rt::test]
async fn returns_404_for_missing_file_under_symlinked_module() {
    let (_store, _scan, config) = mise_layout_fixture();
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(AgentRegistry::<()>::default()))
            .app_data(web::Data::new(config.clone()))
            .configure(|cfg| configure(cfg, &config)),
    )
    .await;

    let req = test::TestRequest::get()
        .uri("/modules/onnxruntime-web/dist/does-not-exist.js")
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}
