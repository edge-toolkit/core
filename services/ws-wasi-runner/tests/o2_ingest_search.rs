//! End-to-end proof that the pinned `http:openobserve` binary ingests real OTLP and serves it back.
//!
//! Sibling to `vector_otlp_relay.rs`: both spawn a real observability binary and push a span through the
//! OpenTelemetry SDK's OTLP/HTTP exporter (`et-test-otlp`, the same `opentelemetry-otlp` transport the
//! services use via `et-otlp`). Here the subject is the o2 server the `o2` / `o2-native` tasks run, so a
//! version bump that breaks OTLP ingestion or search is caught by the suite, not only by manual checking:
//!
//!   1. Reserve free HTTP + gRPC ports and a temp data dir, then spawn `openobserve` bound to them, fully
//!      isolated from any dev instance (embedded sqlite meta store, telemetry phone-home off).
//!   2. Wait for the server to report healthy on `/healthz`.
//!   3. Emit one span -- authenticated with o2's root basic auth, exactly as `et-otlp` authenticates -- to
//!      o2's OTLP traces endpoint (`/api/{org}/v1/traces`). The span name is a unique marker.
//!   4. Poll o2's trace search API until that span comes back, and assert it survived the round-trip.
//!
//! `openobserve` logs to stdout (unlike vector, which uses stderr), so diagnostics come from
//! [`drain_stdout`]. The binary is installed on every supported platform, so this test is not os-gated:
//! CI decides per lane, per the repo's platform policy.

#![cfg(test)]
#![expect(clippy::single_call_fn, reason = "test code: named single-use step helpers")]

use std::collections::HashMap;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use command_error::CommandExt as _;
use edge_toolkit::auth::BasicAuth;
use et_test_helpers::{ChildGuard, drain_stdout, reserve_port, wait_for_port};
use retry::delay::Fixed;
use retry::retry;
use secrecy::SecretString;

// Ephemeral root credentials for the throwaway instance this test spawns. It is the test's own isolated
// single-node OpenObserve, not the shared dev backend, so these deliberately need not match config/o2.env.
// The password is kept policy-strong so the test keeps passing if a future o2 bump enforces complexity.
const ROOT_EMAIL: &str = "root@o2-roundtrip.test";
const ROOT_PASSWORD: &str = "Complexpass#123";

const ORG: &str = "default";
const SERVICE_NAME: &str = "o2-roundtrip-test";
// The span name doubles as a unique marker so the trace search matches only this run's span.
const MARKER: &str = "o2-roundtrip-marker-8f3a1c";

#[test]
fn openobserve_serves_back_a_span_ingested_over_otlp() {
    // 1. Reserve both ports openobserve binds (HTTP for the API, gRPC for its internal cluster listener)
    //    and give it a private temp data dir, so nothing collides with a local dev o2 or a parallel test.
    let http_port = reserve_port();
    let grpc_port = reserve_port();
    let data_dir = tempfile::tempdir().unwrap();

    let mut child = Command::new("openobserve")
        .env("ZO_ROOT_USER_EMAIL", ROOT_EMAIL)
        .env("ZO_ROOT_USER_PASSWORD", ROOT_PASSWORD)
        .env("ZO_HTTP_PORT", http_port.to_string())
        .env("ZO_GRPC_PORT", grpc_port.to_string())
        .env("ZO_DATA_DIR", data_dir.path())
        // No usage phone-home from a test instance, so a blocked network never adds latency or flake.
        .env("ZO_TELEMETRY", "false")
        .env("RUST_LOG", "warn")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn_checked()
        .unwrap()
        .into_child();
    // openobserve logs to stdout; drain it so diagnostics are available on failure (populated at EOF, i.e.
    // once the child is shut down -- see stop_and_read).
    let log = drain_stdout(&mut child);
    // Kill + reap the server on every exit path, including panics, so no process leaks.
    let mut server = ChildGuard::new(child);

    // 2. The port opening is necessary but not sufficient; wait for /healthz to actually report ok.
    assert!(
        wait_for_port(http_port),
        "openobserve never bound its HTTP port :{http_port}\n{}",
        stop_and_read(&mut server, &log),
    );
    let base = format!("http://127.0.0.1:{http_port}");
    let client = reqwest::blocking::Client::new();
    assert!(
        wait_for_healthy(&client, &base),
        "openobserve never became healthy on :{http_port}\n{}",
        stop_and_read(&mut server, &log),
    );

    // 3. Emit one span over real OTLP/HTTP, authenticated with o2's root basic auth (the same header
    //    et-otlp builds). o2 ingests it into its traces stream.
    let mut headers = HashMap::new();
    BasicAuth::new(ROOT_EMAIL.to_owned(), SecretString::from(ROOT_PASSWORD.to_owned()))
        .add_basic_auth_header(&mut headers);
    et_test_otlp::emit_span(&format!("{base}/api/{ORG}/v1/traces"), headers, SERVICE_NAME, MARKER);

    // 4. Search o2's traces until the span surfaces, then verify it came back carrying our marker + service.
    let Some(hit) = wait_for_marker(&client, &base) else {
        panic!(
            "span with marker {MARKER:?} never came back from o2's trace search API\n{}\n{}",
            search_diagnostics(&client, &base),
            stop_and_read(&mut server, &log),
        );
    };
    let hit_text = hit.to_string();
    assert!(hit_text.contains(MARKER), "trace hit is missing the marker: {hit}");
    assert!(
        hit_text.contains(SERVICE_NAME),
        "trace hit is missing the service name: {hit}"
    );
}

/// Poll `/healthz` until it returns HTTP 200, up to ~60s (o2 is a cold, heavy start on CI runners).
#[must_use]
fn wait_for_healthy(client: &reqwest::blocking::Client, base: &str) -> bool {
    retry(Fixed::from_millis(500).take(120), || {
        match client.get(format!("{base}/healthz")).send() {
            Ok(resp) if resp.status().is_success() => Ok(()),
            _ => Err(()),
        }
    })
    .is_ok()
}

/// Poll o2's trace search API until a hit carrying [`MARKER`] appears, returning it; ~60s ceiling.
///
/// The query window is now +/- 1 hour (o2 timestamps are microseconds since the epoch), wide enough to cover
/// clock skew and the ingest-to-searchable lag without matching anything but this run's span.
#[must_use]
fn wait_for_marker(client: &reqwest::blocking::Client, base: &str) -> Option<serde_json::Value> {
    retry(Fixed::from_millis(500).take(120), || {
        let body = trace_search(client, base)?;
        body["hits"]
            .as_array()
            .and_then(|hits| hits.iter().find(|hit| hit.to_string().contains(MARKER)).cloned())
            .ok_or(())
    })
    .ok()
}

/// Run one trace search over the `default` traces stream; `Ok` only on a parseable success response.
fn trace_search(client: &reqwest::blocking::Client, base: &str) -> Result<serde_json::Value, ()> {
    let now_us = now_micros();
    let one_hour_us = 3_600_000_000_i64;
    let query = serde_json::json!({
        "query": {
            "sql": "SELECT * FROM \"default\"",
            "start_time": now_us.saturating_sub(one_hour_us),
            "end_time": now_us.saturating_add(one_hour_us),
            "from": 0_u64,
            "size": 100_u64,
        }
    });
    // A freshly-created stream can 404 the first query or two before its schema registers; the caller
    // retries, so any transport error, non-success status, or unparsable body is just "not yet".
    let Ok(resp) = client
        .post(format!("{base}/api/{ORG}/_search?type=traces"))
        .basic_auth(ROOT_EMAIL, Some(ROOT_PASSWORD))
        .header("content-type", "application/json")
        .body(query.to_string())
        .send()
    else {
        return Err(());
    };
    if !resp.status().is_success() {
        return Err(());
    }
    let Ok(text) = resp.text() else { return Err(()) };
    let Ok(body) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Err(());
    };
    Ok(body)
}

/// On failure, gather what o2 knows: its trace stream list and one raw trace-search response body.
fn search_diagnostics(client: &reqwest::blocking::Client, base: &str) -> String {
    let streams = client
        .get(format!("{base}/api/{ORG}/streams?type=traces"))
        .basic_auth(ROOT_EMAIL, Some(ROOT_PASSWORD))
        .send()
        .and_then(reqwest::blocking::Response::text)
        .unwrap_or_else(|error| format!("<streams request failed: {error}>"));
    let search =
        trace_search(client, base).map_or_else(|()| "<trace search failed>".to_owned(), |body| body.to_string());
    format!("--- o2 trace streams ---\n{streams}\n--- o2 trace search ---\n{search}")
}

/// Current time in microseconds since the epoch, the unit o2's search-window bounds use.
fn now_micros() -> i64 {
    i64::try_from(SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_micros()).unwrap()
}

/// Shut the server down so its stdout drainer reaches EOF, then return the captured log for diagnostics.
fn stop_and_read(server: &mut ChildGuard, log: &Mutex<String>) -> String {
    server.shutdown();
    // Give the drainer thread a moment to flush the final bytes after EOF.
    std::thread::sleep(Duration::from_millis(200));
    format!("--- openobserve stdout ---\n{}", log.lock().unwrap())
}
