#![cfg(test)]

use actix_web::{App, test, web};
use edge_toolkit::config::{Language, mise_env_includes};
use edge_toolkit::ws_server::AgentRegistry;
use et_modules_service::{ModulesConfig, configure};

#[actix_rt::test]
async fn list_modules_api() {
    // The default search paths, but the root named outright: the server has no default for which module is
    // a deployment's front page, so every caller says which one it means.
    let mut config = ModulesConfig::default();
    config.root = "@edge-toolkit/et-ws-server-static".to_string();
    let config = config;
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(AgentRegistry::<()>::default()))
            .app_data(web::Data::new(config.clone()))
            .configure(|cfg| configure(cfg, &config)),
    )
    .await;

    let req = test::TestRequest::get().uri("/modules/").to_request();
    let resp: Vec<String> = test::call_and_read_body_json(&app, req).await;

    // Modules whose `pkg/` is built by an always-loaded task (the wasm agent in
    // the base config) or shipped as a static directory (the page module and the
    // rclone-downloaded model). These are unconditionally expected.
    //
    // Named with the owner scope because that is what their `package.json` declares and what the server
    // serves: it reshapes no name, so what a registry would accept is what a request has to ask for.
    assert!(resp.contains(&"@edge-toolkit/et-ws-server-static".to_string()));
    assert!(resp.contains(&"@edge-toolkit/et-ws-wasm-agent".to_string()));
    assert!(resp.contains(&"@edge-toolkit/et-model-har-motion1".to_string()));

    // The remaining modules each live in a per-language env: their
    // `build-ws-*-module` task is loaded only when MISE_ENV includes that
    // env, so the `pkg/` won't exist (and the module won't be listed) when
    // CI narrows MISE_ENV. Gate each assertion on the matching env.
    for (module, language) in [
        ("@edge-toolkit/et-ws-comm1", Language::Rust),
        ("@edge-toolkit/et-ws-data1", Language::Rust),
        ("@edge-toolkit/et-ws-har1", Language::Js),
        ("@edge-toolkit/et-ws-face-detection", Language::Js),
    ] {
        if mise_env_includes(language) {
            assert!(resp.contains(&module.to_string()), "missing {module}: {resp:?}");
        }
    }

    // onnxruntime-web and stats-gl are staged by the `js` env (npm:onnxruntime-web
    // and npm:stats-gl in config.js.toml); when MISE_ENV doesn't load js, the
    // packages aren't on disk and the modules listing doesn't include them.
    if mise_env_includes(Language::Js) {
        assert!(resp.contains(&"onnxruntime-web".to_string()));
        assert!(resp.contains(&"stats-gl".to_string()));
    }
}
