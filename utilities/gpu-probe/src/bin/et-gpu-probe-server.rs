//! Run the GPU probe collection server: parse the arguments, settle the certificate, and serve until stopped.

use std::net::IpAddr;

use actix_web::{App, HttpServer, web};
use clap::Parser as _;
use et_gpu_probe_server::registry::Registry;
use et_gpu_probe_server::{Args, advertised_names, app};
use et_https_listener::{net, tls};
use tracing::info;

#[actix_web::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_unset| "info".into());
    tracing_subscriber::fmt().with_env_filter(filter).init();
    let args = Args::parse();
    args.check()?;
    let registry = web::Data::new(Registry::new(args.registry.clone()));
    let root = args.root.clone();
    let server = HttpServer::new(move || {
        App::new().configure(|services| app::configure(services, registry.clone(), root.clone()))
    });
    info!("adding submissions to {}", args.registry.display());
    if args.http {
        info!("open http://localhost:{}/", args.port);
        server.bind((args.bind, args.port))?.run().await?;
        return Ok(());
    }
    let lan: Vec<IpAddr> = net::candidate_ipv4s(None)?
        .into_iter()
        .map(|(_, address)| address)
        .collect();
    let names = advertised_names(&args.public_host, &lan);
    let (cert, key) = tls::load_or_generate_tls_certs(&args.cert, &args.key, &names);
    let server = server.bind_rustls_0_23((args.bind, args.port), tls::build_tls_server_config(cert, key))?;
    info!("open https://localhost:{}/", args.port);
    if args.bind.is_unspecified() {
        for name in &names {
            info!("from another device: https://{name}:{}/", args.port);
        }
    }
    server.run().await?;
    Ok(())
}
