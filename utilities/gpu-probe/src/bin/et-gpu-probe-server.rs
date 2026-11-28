//! Run the GPU probe collection server: parse the arguments, settle the certificate, and serve until stopped.

use std::net::{IpAddr, SocketAddr};

use actix_web::{App, HttpServer, web};
use clap::Parser as _;
use et_gpu_probe_server::registry::RegistryTree;
use et_gpu_probe_server::{Args, advertised_names, app};
use et_https_listener::{net, tls};
use tracing::info;

#[actix_web::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_unset| "info".into());
    tracing_subscriber::fmt().with_env_filter(filter).init();
    let args = Args::parse();
    args.check()?;
    let registry = web::Data::new(RegistryTree::new(args.registry.clone()));
    let root = args.root.clone();
    let server = HttpServer::new(move || {
        App::new().configure(|services| app::configure(services, registry.clone(), root.clone()))
    });
    info!("adding submissions to {}", args.registry.display());
    let lan: Vec<IpAddr> = net::candidate_ipv4s(None)?
        .into_iter()
        .map(|(_, address)| address)
        .collect();
    // The server is opened at the address it is bound to, unless that is loopback or every address, which localhost
    // reaches. A socket address writes an IPv6 host in the brackets a URL needs.
    let opened = if args.bind.is_loopback() || args.bind.is_unspecified() {
        format!("localhost:{}", args.port)
    } else {
        SocketAddr::new(args.bind, args.port).to_string()
    };
    if args.http {
        let server = server.bind((args.bind, args.port))?;
        info!("open http://{opened}/");
        if args.bind.is_unspecified() {
            for address in &lan {
                info!("from another device, with no WebGPU: http://{address}:{}/", args.port);
            }
        }
        server.run().await?;
        return Ok(());
    }
    let names = advertised_names(&args.public_host, &lan);
    let (cert, key) = tls::load_or_generate_tls_certs(&args.cert, &args.key, &names);
    let server = server.bind_rustls_0_23((args.bind, args.port), tls::build_tls_server_config(cert, key))?;
    info!("open https://{opened}/");
    if args.bind.is_unspecified() {
        for name in &names {
            info!("from another device: https://{name}:{}/", args.port);
        }
    }
    server.run().await?;
    Ok(())
}
