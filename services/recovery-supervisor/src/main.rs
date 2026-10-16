//! Recovery service entrypoint; policy and backend behavior are exercised through the library.

use et_recovery_supervisor::config::EnvConfig;
use et_recovery_supervisor::kubernetes::Kubernetes;

/// Load environment configuration and run recovery supervision.
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let config = serde_env::from_env::<EnvConfig>()?.recovery;
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init()?;
    let backend = Kubernetes::new(config.clone())?;
    et_recovery_supervisor::runtime::run(config, backend).await?;
    Ok(())
}
