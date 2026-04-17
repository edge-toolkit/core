use std::path::PathBuf;

use actix_web::middleware::Logger;
use actix_web::{App, HttpServer, web};
use clap::Parser;
use et_ws_server::{AgentRegistry, browser_static_dir, configure_app, wasm_modules_dir, wasm_pkg_dir, workspace_root};
use opentelemetry::global;
use opentelemetry_sdk::trace::SdkTracerProvider as TracerProvider;
use tracing::{error, info};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Path to agent registry YAML file
    #[arg(short, long, default_value = "registry.yaml")]
    agent_registry: PathBuf,
}

// Initialize OpenTelemetry
fn init_tracing() -> opentelemetry_sdk::trace::SdkTracerProvider {
    let provider = TracerProvider::builder().build();
    global::set_tracer_provider(provider.clone());
    provider
}

fn tls_config() -> std::io::Result<rustls::ServerConfig> {
    let certified = rcgen::generate_simple_self_signed(vec![
        "localhost".to_string(),
        "127.0.0.1".to_string(),
        "::1".to_string(),
    ])
    .map_err(|e| std::io::Error::other(format!("failed to generate dev certificate: {e}")))?;

    let cert_der: rustls::pki_types::CertificateDer<'static> = certified.cert.der().clone();
    let key_der = rustls::pki_types::PrivatePkcs8KeyDer::from(certified.signing_key.serialize_der());

    rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert_der], key_der.into())
        .map_err(|e| std::io::Error::other(format!("failed to configure TLS: {e}")))
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    let args = Args::parse();
    let _provider = init_tracing();

    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,ws_server=debug,actix_web=info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let tls_config = tls_config()?;

    let network_ip = local_ip_address::local_ip()
        .map(|ip| ip.to_string())
        .unwrap_or_else(|_| "127.0.0.1".to_string());
    let https_url = format!("https://{}:8443", network_ip);
    info!("Starting WebSocket server on http://{}:8080", network_ip);
    info!("Starting WebSocket server on {}", https_url);
    info!("Scan this QR code to open the browser interface:");
    if let Err(e) = qr2term::print_qr(&https_url) {
        error!("Failed to generate QR code: {}", e);
    }
    info!("Serving browser assets from {:?}", browser_static_dir());
    info!("Serving wasm package from {:?}", wasm_pkg_dir());
    info!("Serving wasm modules from {:?}", wasm_modules_dir());
    info!("HTTPS uses an in-memory self-signed localhost certificate for development");

    let agent_registry = web::Data::new(AgentRegistry::load(&args.agent_registry)?);
    let registry_clone = agent_registry.clone();
    let registry_path = args.agent_registry.clone();

    let storage_dir = workspace_root().join("services/ws-server/storage");
    std::fs::create_dir_all(&storage_dir)?;

    let server = HttpServer::new(move || {
        let registry = agent_registry.clone();
        let storage = storage_dir.clone();
        App::new()
            .wrap(Logger::default())
            .configure(|cfg| configure_app(cfg, registry, storage))
    })
    .bind(("0.0.0.0", 8080))?
    .bind_rustls_0_23(("0.0.0.0", 8443), tls_config)?
    .run();

    let handle = server.handle();
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.unwrap();
        info!("Shutdown signal received, saving registry...");
        if let Err(e) = registry_clone.save(&registry_path) {
            error!("Failed to save registry on shutdown: {}", e);
        }
        handle.stop(true).await;
    });

    server.await
}
