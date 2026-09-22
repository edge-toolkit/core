//! Holds the repository's own collector credentials to the values the generated deployments are built from.
//!
//! A generated deployment is self-consistent on its own: the collector's account and the hub's OTLP account
//! both come from `COLLECTOR_USERNAME`, and the password is derived per scenario into one file both halves
//! read. The repository's own dev stack is not -- `config/o2.env` starts the collector and
//! `.mise/config.toml` configures the server, and the two agree only because someone typed the same strings
//! into both. Disagreeing costs nothing at startup and everything afterwards: the collector comes up healthy,
//! the server exports happily, and every span is rejected with a 401 nothing surfaces.
//!
//! So these read the files rather than restating the values, and a rotation in one place fails here instead
//! of in whatever is being debugged a week later.
#![cfg(test)]

use std::path::PathBuf;

use edge_toolkit::config::get_project_root;
use et_cli::COLLECTOR_USERNAME;
use fs_err as fs;

/// Read `config/o2.env` the way docker's `--env-file` does: everything after the first `=` is the value.
///
/// Deliberately not a dotenv parser. That file is consumed by `--env-file` and by `.`-sourcing it in a task
/// body, neither of which strips quotes or honours inline comments, so a parser that did either would accept
/// a file the collector then reads differently.
fn o2_env_value(key: &str) -> String {
    let path = get_project_root().join("config/o2.env");
    let text = fs::read_to_string(&path).unwrap();
    text.lines()
        .find_map(|line| line.strip_prefix(key)?.strip_prefix('='))
        .unwrap_or_else(|| panic!("no {key} in {}", path.display()))
        .to_string()
}

/// Read one `[tasks.ws-server.env]` value, which is how the repository's own server is configured.
fn ws_server_env_value(key: &str) -> String {
    let path: PathBuf = get_project_root().join(".mise/config.toml");
    let text = fs::read_to_string(&path).unwrap();
    let config: toml::Table = toml::from_str(&text).unwrap();
    config
        .get("tasks")
        .and_then(|tasks| tasks.get("ws-server"))
        .and_then(|task| task.get("env"))
        .and_then(|env| env.get(key))
        .and_then(toml::Value::as_str)
        .unwrap_or_else(|| panic!("no {key} under [tasks.ws-server.env] in {}", path.display()))
        .to_string()
}

#[test]
fn the_dev_collector_account_is_the_one_generated_deployments_use() {
    // One account seen from both ends: `ZO_ROOT_USER_EMAIL` creates it, `OTLP_AUTH_USERNAME` presents it.
    assert_eq!(
        o2_env_value("ZO_ROOT_USER_EMAIL"),
        COLLECTOR_USERNAME,
        "the collector this repository starts must use the account the generated deployments name"
    );
    assert_eq!(
        ws_server_env_value("OTLP_AUTH_USERNAME"),
        COLLECTOR_USERNAME,
        "and the server that exports to it must present that same account"
    );
}

#[test]
fn the_dev_collector_password_matches_on_both_sides() {
    // The other half of the same credential, which has no constant to hang from: it is a committed dev-only
    // password rather than anything a generated deployment reuses, so the two files are its only statement of
    // it and agreeing with each other is the whole of what can be checked.
    assert_eq!(
        ws_server_env_value("OTLP_AUTH_PASSWORD"),
        o2_env_value("ZO_ROOT_USER_PASSWORD"),
        "the server authenticates as the collector's root user, so the two passwords are one value"
    );
}
