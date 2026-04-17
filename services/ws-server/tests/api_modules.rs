use std::path::PathBuf;

use actix_web::{App, test, web};
use et_ws_server::{AgentRegistry, configure_app};

#[actix_rt::test]
async fn test_list_modules() {
    // We need to use the real modules dir or a mock one.
    // list_modules uses wasm_modules_dir() which is hardcoded in lib.rs to workspace_root().join("services/ws-modules")

    let agent_registry = web::Data::new(AgentRegistry::default());
    let storage_dir = PathBuf::from("/tmp/et-ws-test-storage");

    let app =
        test::init_service(App::new().configure(|cfg| configure_app(cfg, agent_registry.clone(), storage_dir.clone())))
            .await;

    let req = test::TestRequest::get().uri("/api/modules").to_request();
    let resp: Vec<String> = test::call_and_read_body_json(&app, req).await;

    // We expect at least the modules we know exist
    assert!(resp.contains(&"comm1".to_string()));
    assert!(resp.contains(&"data1".to_string()));
    assert!(resp.contains(&"har1".to_string()));
    assert!(resp.contains(&"face-detection".to_string()));
}
