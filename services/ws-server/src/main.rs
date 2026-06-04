#![expect(
    clippy::print_stderr,
    clippy::unwrap_used,
    clippy::use_debug,
    reason = "server entry point: bootstrap crashes are intentional; eprintln! + Debug env dump precede tracing setup"
)]

use std::path::PathBuf;

use actix_web::middleware::{DefaultHeaders, Logger};
use actix_web::{App, HttpServer, web};
use clap::Parser;
use et_modules_service::list_modules;
use et_ws_server::config::Config;
use et_ws_server::configure_app;
use et_ws_service::load_registry;
use tracing::{error, info};
use tracing_actix_web::TracingLogger;
use tracing_subscriber::{layer::SubscriberExt as _, util::SubscriberInitExt as _};

mod tls;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Path to agent registry YAML file.
    #[arg(short, long, default_value = "registry.yaml")]
    agent_registry: PathBuf,
}

#[actix_web::main]
async fn main() -> Result<(), std::io::Error> {
    let args = Args::parse();

    let env = serde_env::from_env::<Config>().unwrap();

    eprintln!("Starting with env vars {env:#?}");

    #[expect(
        clippy::option_if_let_else,
        reason = "both branches log and configure distinct tracing subscribers; map_or_else hides the structure"
    )]
    let otel_handles = if let Some(otlp_config) = &env.otlp {
        info!("OpenTelemetry configuration detected, initializing tracing...");
        Some(et_otlp::init(otlp_config))
    } else {
        info!("No OpenTelemetry configuration detected, using default tracing settings...");
        tracing_subscriber::registry()
            .with(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| "info,et_ws_server=debug".into()),
            )
            .with(tracing_subscriber::fmt::layer())
            .init();
        None
    };

    let network_ip = local_ip_address::local_ip().map_or_else(|_unused| "127.0.0.1".to_string(), |ip| ip.to_string());

    let cert_filename = &env.tls.cert_file;
    let key_filename = &env.tls.key_file;
    let (cert_der, key_der) = if cert_filename.exists() && key_filename.exists() {
        info!("Loading TLS certificate from {:?}", cert_filename);
        tls::load_tls_certs(cert_filename, key_filename)
    } else {
        info!(
            "Generated self-signed localhost certificate to {:?} and key to {:?}",
            cert_filename, key_filename
        );
        tls::generate_tls_certs(cert_filename, key_filename)
    };
    let rustls_config = tls::build_tls_server_config(cert_der, key_der);

    let https_url = format!(
        "https://{}:{}",
        network_ip,
        edge_toolkit::ports::Services::SecureWebSocketServer.port()
    );
    info!(
        "Starting WebSocket server on http://{}:{}",
        network_ip,
        edge_toolkit::ports::Services::InsecureWebSocketServer.port()
    );
    info!("Starting WebSocket server on {}", https_url);
    info!("Scan this QR code to open the browser interface:");
    if let Err(e) = qr2term::print_qr(&https_url) {
        error!("Failed to generate QR code: {}", e);
    }

    let agent_registry = web::Data::new(load_registry(&args.agent_registry).unwrap());
    let registry_clone = agent_registry.clone();
    let registry_path = args.agent_registry.clone();

    fs_err::create_dir_all(&env.storage.path).unwrap();

    for (name, pkg_dir) in list_modules(&env.modules) {
        info!("Loading module {name} at {}", pkg_dir.display());
    }
    let server = HttpServer::new(move || {
        let registry = agent_registry.clone();
        let config = env.clone();
        // `TracingLogger` extracts the W3C `traceparent` header from
        // incoming requests (via the `opentelemetry_0_31` feature) and uses
        // it as the parent context of the per-request span — that's how
        // traces propagate from the wasi-runner (or any client that injects
        // `traceparent`) into the server.
        App::new()
            // `Logger::default()` emits one `actix_web` INFO log line per
            // request (method, path, status, duration). The
            // tracing-subscriber default has `tracing-log` enabled, so the
            // `log` records show up in the same console as tracing events
            // — invaluable when an actix-files 404 would otherwise be
            // silent (TracingLogger only creates the span, it doesn't emit
            // events on success).
            .wrap(Logger::default())
            .wrap(TracingLogger::default())
            .wrap(
                DefaultHeaders::new()
                    .add(("Cross-Origin-Opener-Policy", "same-origin"))
                    .add(("Cross-Origin-Embedder-Policy", "require-corp")),
            )
            .configure(|cfg| configure_app(cfg, registry, &config))
    })
    .bind(("0.0.0.0", edge_toolkit::ports::Services::InsecureWebSocketServer.port()))?
    .bind_rustls_0_23(
        ("0.0.0.0", edge_toolkit::ports::Services::SecureWebSocketServer.port()),
        rustls_config,
    )?
    .run();

    let handle = server.handle();
    let _shutdown_task = tokio::spawn(async move {
        tokio::signal::ctrl_c().await.unwrap();
        info!("Shutdown signal received, saving registry...");
        if let Err(e) = registry_clone.save(&registry_path) {
            error!("Failed to save registry on shutdown: {}", e);
        }
        handle.stop(true).await;
    });

    let result = server.await;
    // Flush batched spans/logs before exit; otherwise short-lived runs lose
    // the tail of the trace.
    if let Some(handles) = otel_handles {
        handles.shutdown();
    }
    result
}
