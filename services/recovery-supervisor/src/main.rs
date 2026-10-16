//! Recovery service entrypoint; policy and backend behavior are exercised through the library.

use clap::Parser as _;
use et_recovery_supervisor::cli::Args;
use et_recovery_supervisor::config::Config;
use et_recovery_supervisor::kubernetes::Kubernetes;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let args = Args::parse();
    let config: Config = toml::from_str(&std::fs::read_to_string(args.config)?)?;
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init()?;
    let backend = Kubernetes::new(config.clone())?;
    et_recovery_supervisor::runtime::run(config, backend).await?;
    Ok(())
}
