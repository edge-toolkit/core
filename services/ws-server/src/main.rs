#![expect(
    clippy::print_stderr,
    clippy::unwrap_used,
    clippy::use_debug,
    reason = "server entry point: bootstrap crashes are intentional; eprintln! + Debug env dump precede tracing setup"
)]

use actix_web::middleware::{DefaultHeaders, Logger};
use actix_web::{App, HttpServer, web};
use clap::Parser as _;
use et_ws_server::cli::Args;
use et_ws_server::config::Config;
use et_ws_server::configure_app;
#[cfg(feature = "tls")]
use et_ws_server::tls;
use et_ws_service::load_registry;
use tracing::{error, info, warn};
use tracing_actix_web::TracingLogger;
use tracing_subscriber::{layer::SubscriberExt as _, util::SubscriberInitExt as _};

#[actix_web::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let args = Args::parse();

    let env = serde_env::from_env::<Config>().unwrap();

    eprintln!("Starting with env vars {env:#?}");

    #[cfg(feature = "otlp")]
    let otel_handles = if let Some(otlp_config) = &env.otlp {
        info!("OpenTelemetry configuration detected, initializing tracing...");
        Some(et_otlp::init(otlp_config)?)
    } else {
        info!("No OpenTelemetry configuration detected, using default tracing settings...");
        init_console_tracing();
        None
    };
    #[cfg(not(feature = "otlp"))]
    init_console_tracing();

    let log_interface = env.net.log_interface.as_deref();
    info!(
        "Selecting advertised IP with log_interface={}",
        log_interface.unwrap_or("(unset)")
    );
    let ip_candidates = et_ws_server::net::candidate_ipv4s(log_interface).inspect_err(|e| error!("{e}"))?;
    let network_ip = ip_candidates
        .first()
        .map_or_else(|| "127.0.0.1".to_string(), |(_, addr)| addr.to_string());
    if ip_candidates.is_empty() {
        warn!("No usable IPv4 candidate found; advertising 127.0.0.1 (see the interface lines above)");
    }

    #[cfg(feature = "tls")]
    let rustls_config = {
        let cert_filename = &env.tls.cert_file;
        let key_filename = &env.tls.key_file;
        let (cert_der, key_der) = if cert_filename.exists() && key_filename.exists() {
            info!("Loading TLS certificate from {:?}", cert_filename);
            tls::load_tls_certs(cert_filename, key_filename)
        } else {
            #[cfg(feature = "tls-self-signed")]
            {
                info!(
                    "Generated self-signed localhost certificate to {:?} and key to {:?}",
                    cert_filename, key_filename
                );
                tls::generate_tls_certs(cert_filename, key_filename)
            }
            #[cfg(not(feature = "tls-self-signed"))]
            return Err(format!(
                "TLS certificate {} or key {} is missing; this build cannot generate one",
                cert_filename.display(),
                key_filename.display()
            )
            .into());
        };
        tls::build_tls_server_config(cert_der, key_der)
    };

    let http_port = edge_toolkit::ports::Services::InsecureWebSocketServer.port();
    #[cfg(feature = "tls")]
    let (scheme, advertised_port) = ("https", edge_toolkit::ports::Services::SecureWebSocketServer.port());
    #[cfg(not(feature = "tls"))]
    let (scheme, advertised_port) = ("http", http_port);
    let advertised_url = format!("{scheme}://{network_ip}:{advertised_port}");
    info!("Starting WebSocket server on http://{}:{}", network_ip, http_port);
    info!("Advertising {}", advertised_url);
    for (interface, addr) in &ip_candidates {
        info!(
            "Reachable via {} at {}://{}:{}",
            interface, scheme, addr, advertised_port
        );
    }
    #[cfg(feature = "qr")]
    {
        info!("Scan this QR code to open the browser interface:");
        if let Err(e) = qr2term::print_qr(&advertised_url) {
            error!("Failed to generate QR code: {}", e);
        }
    }

    let agent_registry = web::Data::new(load_registry(&args.agent_registry).unwrap());
    let registry_clone = agent_registry.clone();
    let registry_path = args.agent_registry.clone();

    #[cfg(feature = "modules")]
    for (name, pkg_dir) in et_modules_service::list_modules(&env.modules) {
        info!("Loading module {name} at {}", pkg_dir.display());
    }
    let server = HttpServer::new(move || {
        let registry = agent_registry.clone();
        let config = env.clone();
        // `TracingLogger` extracts the W3C `traceparent` header from
        // incoming requests (via the `opentelemetry_0_31` feature) and uses
        // it as the parent context of the per-request span -- that's how
        // traces propagate from the wasi-runner (or any client that injects
        // `traceparent`) into the server.
        App::new()
            // `Logger::default()` emits one `actix_web` INFO log line per
            // request (method, path, status, duration). The
            // tracing-subscriber default has `tracing-log` enabled, so the
            // `log` records show up in the same console as tracing events
            // -- invaluable when an actix-files 404 would otherwise be
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
    .bind(("0.0.0.0", http_port))?;
    #[cfg(feature = "tls")]
    let server = server.bind_rustls_0_23(("0.0.0.0", advertised_port), rustls_config)?;
    let server = server.run();

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
    #[cfg(feature = "otlp")]
    if let Some(handles) = otel_handles {
        handles.shutdown();
    }
    result?;
    Ok(())
}

#[expect(
    clippy::single_call_fn,
    reason = "the console fallback is reached from the `otlp` and non-`otlp` builds alike; one body serves both"
)]
fn init_console_tracing() {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info,et_ws_server=debug".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();
}
