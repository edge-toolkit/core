use std::path::PathBuf;

use actix_web::middleware::{DefaultHeaders, Logger};
use actix_web::{App, HttpServer, web};
use clap::Parser;
use et_modules_service::list_modules;
use et_ws_server::config::Config;
use et_ws_server::configure_app;
use et_ws_service::load_registry;
use tracing::{error, info};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

mod otlp;
mod tls;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Path to agent registry YAML file
    #[arg(short, long, default_value = "registry.yaml")]
    agent_registry: PathBuf,
}

#[actix_web::main]
async fn main() -> Result<(), std::io::Error> {
    let args = Args::parse();

    let env = serde_env::from_env::<Config>().unwrap();

    eprintln!("Starting with env vars {env:#?}");

    if let Some(otlp_config) = &env.otlp {
        info!("OpenTelemetry configuration detected, initializing tracing...");
        let _provider = crate::otlp::init(otlp_config);
    } else {
        info!("No OpenTelemetry configuration detected, using default tracing settings...");
        tracing_subscriber::registry()
            .with(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| "info,et_ws_server=debug".into()),
            )
            .with(tracing_subscriber::fmt::layer())
            .init();
    }

    let network_ip = local_ip_address::local_ip()
        .map(|ip| ip.to_string())
        .unwrap_or_else(|_| "127.0.0.1".to_string());

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

    std::fs::create_dir_all(&env.storage.path).unwrap();

    for (name, pkg_dir) in list_modules(&env.modules) {
        info!("Loading module {name} at {}", pkg_dir.display());
    }
    let server = HttpServer::new(move || {
        let registry = agent_registry.clone();
        let config = env.clone();
        App::new()
            .wrap(Logger::default())
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
