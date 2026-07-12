//! `WS_MAX_FRAME_SIZE` deserialization, through the real `serde_env::from_env`
//! path the ws-server uses, with the process env set via `temp-env`. Verifies
//! the human-byte-size parsing (`64MiB`, plain byte counts) and the 64 MiB
//! default when the variable is absent.
#![cfg(test)]
#![expect(
    clippy::decimal_literal_representation,
    reason = "test code: byte sizes read clearer as decimal MiB math than hex"
)]

use et_ws_service::WsConfig;
use serde::Deserialize;

// Mirror the ws-server's nesting so the env key is `WS_MAX_FRAME_SIZE`.
#[derive(Debug, Deserialize)]
struct Wrapper {
    #[serde(default)]
    ws: WsConfig,
}

fn load() -> WsConfig {
    serde_env::from_env::<Wrapper>().unwrap().ws
}

#[test]
fn max_frame_size_absent_defaults_to_64_mib() {
    temp_env::with_var_unset("WS_MAX_FRAME_SIZE", || {
        assert_eq!(load().max_frame_size, 64 * 1024 * 1024);
    });
}

#[test]
fn max_frame_size_parses_human_size() {
    temp_env::with_var("WS_MAX_FRAME_SIZE", Some("32MiB"), || {
        assert_eq!(load().max_frame_size, 32 * 1024 * 1024);
    });
}

#[test]
fn max_frame_size_parses_plain_byte_count() {
    temp_env::with_var("WS_MAX_FRAME_SIZE", Some("1048576"), || {
        assert_eq!(load().max_frame_size, 1_048_576);
    });
}

#[test]
fn connection_timeout_absent_defaults_to_15s() {
    temp_env::with_var_unset("WS_CONNECTION_TIMEOUT", || {
        assert_eq!(load().connection_timeout, Some(std::time::Duration::from_secs(15)));
    });
}

#[test]
fn connection_timeout_parses_humantime() {
    temp_env::with_var("WS_CONNECTION_TIMEOUT", Some("1m30s"), || {
        assert_eq!(load().connection_timeout, Some(std::time::Duration::from_secs(90)));
    });
}

#[test]
fn connection_timeout_none_sentinel_disables() {
    temp_env::with_var("WS_CONNECTION_TIMEOUT", Some("none"), || {
        assert_eq!(load().connection_timeout, None);
    });
}
