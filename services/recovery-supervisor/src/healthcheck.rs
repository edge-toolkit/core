//! Exit successfully only when the configured supervisor owns its lease and has completed a recent cycle.

use et_recovery_supervisor::config::EnvConfig;

/// Load the configured job and validate its local supervision heartbeat.
fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let config = serde_env::from_env::<EnvConfig>()?.recovery;
    et_recovery_supervisor::health::check(&config)?;
    Ok(())
}
