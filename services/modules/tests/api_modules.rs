#![cfg(test)]

use actix_web::{App, test, web};
use edge_toolkit::config::{Language, mise_env_includes};
use edge_toolkit::ws_server::AgentRegistry;
use et_modules_service::{ModulesConfig, configure};

#[actix_rt::test]
async fn list_modules_api() {
    let config = ModulesConfig::default();
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(AgentRegistry::<()>::default()))
            .app_data(web::Data::new(config.clone()))
            .configure(|cfg| configure(cfg, &config)),
    )
    .await;

    let req = test::TestRequest::get().uri("/modules/").to_request();
    let resp: Vec<String> = test::call_and_read_body_json(&app, req).await;

    // Modules whose `pkg/` is built by an always-loaded task (et-ws-wasm-agent
    // in the base config) or shipped as a static directory (et-ws-server-static
    // and the rclone-downloaded model). These are unconditionally expected.
    assert!(resp.contains(&"et-ws-server-static".to_string()));
    assert!(resp.contains(&"et-ws-wasm-agent".to_string()));
    assert!(resp.contains(&"et-model-har-motion1".to_string()));

    // The remaining modules each live in a per-language env: their
    // `build-ws-*-module` task is loaded only when MISE_ENV includes that
    // env, so the `pkg/` won't exist (and the module won't be listed) when
    // CI narrows MISE_ENV. Gate each assertion on the matching env.
    for (module, language) in [
        ("et-ws-comm1", Language::Rust),
        ("et-ws-data1", Language::Rust),
        ("et-ws-har1", Language::Js),
        ("et-ws-face-detection", Language::Js),
    ] {
        if mise_env_includes(language) {
            assert!(resp.contains(&module.to_string()), "missing {module}: {resp:?}");
        }
    }

    // onnxruntime-web is staged by the `js` env (npm:onnxruntime-web in
    // config.js.toml); when MISE_ENV doesn't load js, the package isn't on
    // disk and the modules listing doesn't include it.
    if mise_env_includes(Language::Js) {
        assert!(resp.contains(&"onnxruntime-web".to_string()));
    }
}
