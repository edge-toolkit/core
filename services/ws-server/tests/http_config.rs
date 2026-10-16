//! The hub reads its actix `HttpServer` settings from `HTTP_*` variables.
#![cfg(test)]

use std::time::Duration;

use et_actix_web_config::Workers;
use et_ws_server::config::Config;

#[test]
fn http_variables_reach_the_hub_config() {
    let config: Config = serde_env::from_iter([("HTTP_WORKERS", "3"), ("HTTP_SHUTDOWN_TIMEOUT", "20")]).unwrap();

    assert_eq!(config.http.workers.map(Workers::into_inner), Some(3));
    assert_eq!(config.http.shutdown_timeout, Some(Duration::from_secs(20)));
}
