use actix_web::{App, test, web};
use edge_toolkit::ws_server::{AgentRegistry, Config};
use et_modules_service::configure;

#[actix_rt::test]
async fn list_modules_api() {
    let config = Config::default();
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(AgentRegistry::<()>::default()))
            .app_data(web::Data::new(config.clone()))
            .configure(|cfg| configure(cfg, &config)),
    )
    .await;

    let req = test::TestRequest::get().uri("/modules/").to_request();
    let resp: Vec<String> = test::call_and_read_body_json(&app, req).await;

    assert!(resp.contains(&"et-ws-server-static".to_string()));
    assert!(resp.contains(&"et-ws-wasm-agent".to_string()));
    assert!(resp.contains(&"et-ws-comm1".to_string()));
    assert!(resp.contains(&"et-ws-data1".to_string()));
    assert!(resp.contains(&"et-ws-har1".to_string()));
    assert!(resp.contains(&"et-ws-face-detection".to_string()));
    assert!(resp.contains(&"et-model-har-motion1".to_string()));
    assert!(resp.contains(&"onnxruntime-web".to_string()));
}
