//! Round-trips the storage service against a real S3 server to prove the non-default backend works.
//!
//! The default backend is local disk and is covered by `put.rs`; this file exercises the other half of
//! `StorageConfig::url` -- that pointing it at `s3://<bucket>` makes the same `PUT`/`GET` routes read and write
//! through `object_store`'s S3 client instead, with no change to the wire protocol.
//!
//! The server under test is `rustfs`, a Rust S3 implementation installed as a mise tool. Two things about it
//! shape this test, both established by probing it directly:
//!
//! 1. It does **not** auto-create buckets -- a `PUT` into an unknown bucket is a 404. It does, however, adopt a
//!    directory that already exists in its volume, so the bucket is created with `mkdir` before startup rather
//!    than by a signed `CreateBucket` call. That keeps the test free of an HTTP client and a `SigV4` signer.
//! 2. Objects are stored erasure-coded as `<volume>/<bucket>/<key>/xl.meta`, so the assertion that the bytes
//!    really landed in S3 checks for that directory rather than a plain file.
//!
//! Credentials reach `build_store` through the conventional `AWS_*` variables, which it forwards to
//! `object_store` as options -- so this also covers the mechanism that lets an operator supply secrets as
//! separate environment variables. They are scoped with `temp_env` so they never leak into another test in the
//! same process.

#![cfg(test)]

use std::collections::BTreeMap;
use std::path::Path;
use std::process::{Command, Stdio};

use actix_web::http::StatusCode;
use actix_web::{App, test, web};
use command_error::CommandExt as _;
use edge_toolkit::ws::AgentConnectionState;
use edge_toolkit::ws_server::{AgentRecord, AgentRegistry};
use et_storage_service::{StorageConfig, configure};
use et_test_helpers::{ChildGuard, reserve_port, wait_for_port};

/// Single bucket holding every agent's objects, keyed `<agent_id>/<filename>`.
///
/// One bucket rather than one per agent keeps the key layout identical to the local-disk backend, so the same
/// `object_path` works unchanged for both.
const BUCKET: &str = "et-storage";
const AGENT: &str = "agent-s3";
const FILENAME: &str = "payload.txt";
const BODY: &[u8] = b"bytes that must survive a round-trip through S3";

/// rustfs requires credentials, so the test supplies throwaway ones and hands the same pair to `object_store`.
const ACCESS_KEY: &str = "et-test-access-key";
const SECRET_KEY: &str = "et-test-secret-key";

#[expect(
    clippy::single_call_fn,
    reason = "mirrors put.rs's helper of the same name; kept for symmetry"
)]
fn registry_with_agent(agent_id: &str) -> AgentRegistry<()> {
    let mut agents = BTreeMap::new();
    let _inserted = agents.insert(
        agent_id.to_string(),
        AgentRecord::new(AgentConnectionState::Connected, None, Some(())),
    );
    AgentRegistry::from_agents(agents)
}

/// Start `rustfs` on `port` serving `volume`, and wait for it to accept connections.
///
/// Spawned by bare name so the mise-managed binary on `PATH` is used. If it is missing the `expect` below fails
/// loudly -- a missing mise tool is a misconfigured environment, not a reason for this test to quietly pass.
#[expect(
    clippy::expect_used,
    clippy::single_call_fn,
    reason = "distinct setup step, and the expect message names the missing mise tool an unwrap would hide"
)]
fn start_rustfs(volume: &Path, port: u16) -> ChildGuard {
    let child = Command::new("rustfs")
        .arg("server")
        .arg(volume)
        .arg("--address")
        .arg(format!("127.0.0.1:{port}"))
        .arg("--access-key")
        .arg(ACCESS_KEY)
        .arg("--secret-key")
        .arg(SECRET_KEY)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn_checked()
        .expect("rustfs must be on PATH (mise tool `rustfs`)")
        .into_child();
    let guard = ChildGuard::new(child);
    assert!(wait_for_port(port), "rustfs did not start listening on port {port}");
    guard
}

/// The `AWS_*` pairs `object_store`'s S3 builder consumes, pointing it at the local rustfs.
///
/// `AWS_ALLOW_HTTP` is required because the endpoint is plain HTTP; addressing stays path-style, which is
/// `object_store`'s default and the only style rustfs serves unless `--server-domains` is set.
#[expect(
    clippy::single_call_fn,
    reason = "names the env contract under test; kept separate for readability"
)]
fn aws_env(port: u16) -> Vec<(&'static str, Option<String>)> {
    vec![
        ("AWS_ENDPOINT", Some(format!("http://127.0.0.1:{port}"))),
        ("AWS_ACCESS_KEY_ID", Some(ACCESS_KEY.to_string())),
        ("AWS_SECRET_ACCESS_KEY", Some(SECRET_KEY.to_string())),
        ("AWS_DEFAULT_REGION", Some("us-east-1".to_string())),
        ("AWS_ALLOW_HTTP", Some("true".to_string())),
    ]
}

// Skipped on Windows: the pinned rustfs 1.0.0-beta.12 cannot initialise its storage on Windows, so it never
// reaches quorum and answers every request with `503 Service not ready: waiting for storage_quorum` -- which
// object_store retries a handful of times and then surfaces, turning the PUT below into a 500. The rustfs cause
// is a self-inflicted sharing violation: on Windows it opens each guarded ancestor directory (`.rustfs.sys` and
// friends) with FILE_SHARE_READ only, then renames a freshly-written child into that guarded parent, and Windows
// rejects the parent write with ERROR_SHARING_VIOLATION (Win32 code 32) -- logged by rustfs as
//   reliable_rename failed. src_file_path: "...\\.rustfs.sys\\<uuid>", dst_file_path: "...\\.rustfs.sys\\format.json",
//   err: Os { code: 32, ... "The process cannot access the file because it is being used by another process." }
// This is not init-only: the same guarded rename backs the object-commit path (rustfs `rename_all` / `rename_data`),
// so no store-an-object round-trip can pass on Windows with this build -- there is no pre-seed or wait that helps.
// Fixed upstream in rustfs PR https://github.com/rustfs/rustfs/pull/5663 (guarded dirs now share FILE_SHARE_WRITE
// too; related issue https://github.com/rustfs/rustfs/issues/5419), merged 2026-08-03 -- AFTER the beta.12 tag
// (2026-07-30), so no released rustfs contains it yet. Observed on our commit
// 59f4ab6368af5c6824dafe2e11c9c598f16f5334 at
// https://github.com/edge-toolkit/core/actions/runs/30980282749/job/92223040435 (default (windows-latest, 120)).
// Re-enable by dropping this attribute once the `rustfs` mise tool is bumped to a release that includes #5663.
#[actix_rt::test]
#[cfg_attr(
    windows,
    ignore = "rustfs beta.12 cannot init storage on Windows (ERROR_SHARING_VIOLATION) -- see above"
)]
async fn round_trips_put_and_get_through_the_s3_backend() {
    let volume = tempfile::tempdir().unwrap();
    // Pre-create the bucket: rustfs adopts an existing volume directory but will not create one on demand.
    fs_err::create_dir_all(volume.path().join(BUCKET)).unwrap();
    let port = reserve_port();
    let mut rustfs = start_rustfs(volume.path(), port);

    let config = StorageConfig::new(format!("s3://{BUCKET}"));

    let (put_status, get_status, body) = temp_env::async_with_vars(aws_env(port), async {
        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(registry_with_agent(AGENT)))
                .configure(|cfg| configure::<()>(cfg, &config)),
        )
        .await;

        let put = test::TestRequest::put()
            .uri(&format!("/storage/{AGENT}/{FILENAME}"))
            .set_payload(BODY)
            .to_request();
        let put_status = test::call_service(&app, put).await.status();

        let get = test::TestRequest::get()
            .uri(&format!("/storage/{AGENT}/{FILENAME}"))
            .to_request();
        let get_resp = test::call_service(&app, get).await;
        let get_status = get_resp.status();
        let body = test::read_body(get_resp).await;

        (put_status, get_status, body)
    })
    .await;

    rustfs.shutdown();

    assert_eq!(put_status, StatusCode::OK, "PUT through the S3 backend should succeed");
    assert_eq!(get_status, StatusCode::OK, "GET through the S3 backend should succeed");
    assert_eq!(&*body, BODY, "bytes must survive the S3 round-trip unchanged");

    // Proves the S3 backend actually served the request rather than some local fallback: the object exists
    // inside rustfs's own volume, as its erasure-coded object directory.
    let stored = volume.path().join(BUCKET).join(AGENT).join(FILENAME);
    assert!(
        stored.is_dir(),
        "expected the object at {} in rustfs's volume",
        stored.display()
    );
}
