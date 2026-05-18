use std::net::TcpListener;

use actix_web::{App, HttpServer, web};
use et_modules_service::{ModulesConfig, configure as configure_modules};
use et_storage_service::{StorageConfig, configure as configure_storage};
use et_ws_service::{AgentSession, WsAgentRegistry, WsConfig, configure as configure_ws};
use tempfile::TempDir;
use tracing_actix_web::TracingLogger;

/// A running test server. The temporary storage directory is cleaned up on drop.
pub struct TestServer {
    pub base_url: String,
    pub ws_url: String,
    pub storage_dir: TempDir,
}

/// Start an in-process ws-server on a free port with a temporary storage directory.
///
/// Serves modules from the default module paths (same as production).
pub fn start() -> TestServer {
    let storage_dir = TempDir::new().expect("failed to create temp storage dir");
    let storage_path = storage_dir.path().to_path_buf();

    // Bind to port 0 to get a free port, then drop the listener so the server can bind it.
    let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();

    let storage_config = StorageConfig { path: storage_path };
    let modules_config = ModulesConfig::default();
    let addr = format!("127.0.0.1:{port}");

    std::thread::spawn(move || {
        actix_rt::System::new().block_on(async move {
            let registry = web::Data::new(WsAgentRegistry::default());
            let storage = web::Data::new(storage_config);
            let modules = modules_config;
            let ws_config = WsConfig::default();
            HttpServer::new(move || {
                // `TracingLogger` mirrors the real ws-server's pipeline:
                // extracts `traceparent` from incoming requests so server
                // spans are children of the caller's trace.
                App::new()
                    .wrap(TracingLogger::default())
                    .app_data(registry.clone())
                    .app_data(storage.clone())
                    .configure(|cfg| configure_ws(cfg, &ws_config))
                    .configure(|cfg| configure_storage::<AgentSession>(cfg, &storage))
                    .configure(|cfg| configure_modules(cfg, &modules))
            })
            .bind(&addr)
            .unwrap()
            .run()
            .await
            .unwrap();
        });
    });

    for _ in 0..50 {
        if std::net::TcpStream::connect(format!("127.0.0.1:{port}")).is_ok() {
            return TestServer {
                base_url: format!("http://127.0.0.1:{port}"),
                ws_url: format!("ws://127.0.0.1:{port}/ws"),
                storage_dir,
            };
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    panic!("test ws-server did not start within 5 seconds on port {port}");
}
