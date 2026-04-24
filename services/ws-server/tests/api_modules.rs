use actix_web::{App, test, web};
use et_ws_server::config::Config;
use et_ws_server::{AgentRegistry, configure_app};

#[actix_rt::test]
async fn list_modules() {
    let agent_registry = web::Data::new(AgentRegistry::default());

    let app =
        test::init_service(App::new().configure(|cfg| configure_app(cfg, agent_registry.clone(), Config::default())))
            .await;

    let req = test::TestRequest::get().uri("/api/modules").to_request();
    let resp: Vec<String> = test::call_and_read_body_json(&app, req).await;

    // We expect at least the modules we know exist
    assert!(resp.contains(&"et-ws-server-static".to_string()));
    assert!(resp.contains(&"et-ws-wasm-agent".to_string()));
    assert!(resp.contains(&"et-ws-comm1".to_string()));
    assert!(resp.contains(&"et-ws-data1".to_string()));
    assert!(resp.contains(&"et-ws-har1".to_string()));
    assert!(resp.contains(&"et-ws-face-detection".to_string()));
    assert!(resp.contains(&"et-model-har-motion1".to_string()));
    assert!(resp.contains(&"onnxruntime-web".to_string()));
}
