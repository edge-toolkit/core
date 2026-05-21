//! Symlink-aware module discovery + serving.
//!
//! mise's `aube` npm backend lays out `node_modules/.aube/node_modules/<pkg>`
//! as *symlinks* to a content-addressed store. The modules service has to
//! follow those symlinks both when scanning (`list_modules`) and when
//! actix-files serves files out of the discovered package dir. The tests
//! here cover the full chain on a tempdir fixture that mirrors the aube
//! layout. Regressing either half manifests as a 404 on
//! `/modules/onnxruntime-web/dist/ort.min.js` (and similar) which is what
//! these tests pin down.

#![cfg(test)]
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::symlink;
use std::path::PathBuf;

use actix_web::http::StatusCode;
use actix_web::{App, test, web};
use edge_toolkit::ws_server::AgentRegistry;
use et_modules_service::{ModulesConfig, configure, list_modules};
use tempfile::TempDir;

const ORT_BUNDLE: &[u8] = b"// pretend ort.min.js bundle";

/// Build a fixture with `<store>/onnxruntime-web-real/` as the real
/// package, and `<scan>/onnxruntime-web` as a symlink pointing at it.
/// `<scan>` is what we hand to `ModulesConfig::paths`. Returns both
/// tempdirs (kept alive by the caller) plus the modules config.
fn aube_layout_fixture() -> (TempDir, TempDir, ModulesConfig) {
    let store = TempDir::new().unwrap();
    let real_pkg = store.path().join("onnxruntime-web-real");
    fs::create_dir_all(real_pkg.join("dist")).unwrap();
    fs::write(
        real_pkg.join("package.json"),
        r#"{"name":"onnxruntime-web","version":"1.26.0"}"#,
    )
    .unwrap();
    fs::write(real_pkg.join("dist/ort.min.js"), ORT_BUNDLE).unwrap();

    // Also drop an et-ws-server-static stub next to it — `configure`
    // panics if the configured `root` module can't be found, so we
    // satisfy that requirement here too.
    let static_root = store.path().join("et-ws-server-static");
    fs::create_dir_all(&static_root).unwrap();
    fs::write(
        static_root.join("package.json"),
        r#"{"name":"et-ws-server-static","version":"0.0.0"}"#,
    )
    .unwrap();
    fs::write(static_root.join("index.html"), b"<!doctype html>").unwrap();

    let scan = TempDir::new().unwrap();
    symlink(&real_pkg, scan.path().join("onnxruntime-web")).unwrap();
    symlink(&static_root, scan.path().join("et-ws-server-static")).unwrap();

    let config = ModulesConfig {
        paths: vec![scan.path().to_path_buf()],
        root: "et-ws-server-static".to_string(),
    };
    (store, scan, config)
}

// `actix_rt::test` because the file imports `actix_web::test` at module
// scope, which shadows the built-in `#[test]` attribute. The body is
// synchronous; the async wrapper is harmless.
#[actix_rt::test]
async fn list_modules_follows_symlinks_to_package_dirs() {
    let (_store, _scan, config) = aube_layout_fixture();

    let found: Vec<(String, PathBuf)> = list_modules(&config);

    // Both packages — the symlinked target onnxruntime-web and the
    // symlinked stub root module — should be discovered.
    let by_name: std::collections::HashMap<&str, &PathBuf> = found.iter().map(|(n, p)| (n.as_str(), p)).collect();
    let pkg_path = by_name
        .get("onnxruntime-web")
        .expect("symlinked onnxruntime-web should be discovered");

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
    let (_store, _scan, config) = aube_layout_fixture();
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
    let (_store, _scan, config) = aube_layout_fixture();
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
    let (_store, _scan, config) = aube_layout_fixture();
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
