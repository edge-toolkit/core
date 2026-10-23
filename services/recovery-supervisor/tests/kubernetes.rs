//! Exercise the real kubectl subprocess against a loopback API with isolated credentials and discovery cache.
#![cfg(test)]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use et_recovery_supervisor::RecoveryError;
use et_recovery_supervisor::kubernetes::{Backend as _, Kubernetes};
use et_recovery_supervisor::policy::Health;
use et_test_helpers::temp_env;
use fs_err as fs;
use k8s_openapi::api::apps::v1::{Deployment, DeploymentSpec, DeploymentStatus};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use k8s_openapi::apimachinery::pkg::version::Info;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};
use tracing::subscriber::set_default;

/// Timestamp fixture activity without changing the command or request deadlines under investigation.
#[derive(Clone, Copy)]
struct Diagnostics {
    started: Instant,
}

impl Diagnostics {
    /// Keep progress in captured test stderr, including observations made before a command times out.
    fn record(&self, message: impl std::fmt::Display) {
        eprintln!(
            "[kubectl-fixture +{:.3}s] {message}",
            self.started.elapsed().as_secs_f64()
        );
    }

    /// Report the failing command and fixture liveness while retaining the real backend error.
    fn command<T>(&self, stage: &str, result: Result<T, RecoveryError>, fixture: &tokio::task::JoinHandle<()>) -> T {
        let value = result.unwrap_or_else(|error| {
            panic!(
                "kubectl {stage} failed after {:.3}s; fixture_finished={}: {error}",
                self.started.elapsed().as_secs_f64(),
                fixture.is_finished()
            );
        });
        self.record(format_args!("kubectl {stage} completed"));
        value
    }
}

/// Provide just the API discovery and Deployment fields used by kubectl get and rollout restart.
#[expect(
    clippy::single_call_fn,
    reason = "API response fixtures are separate from HTTP transport handling"
)]
fn response(path: &str) -> Value {
    match path {
        "/api" => json!({"kind":"APIVersions", "apiVersion":"v1", "versions":["v1"]}),
        "/api/v1" => json!({"kind":"APIResourceList", "apiVersion":"v1", "groupVersion":"v1", "resources":[]}),
        "/apis" => json!({"kind":"APIGroupList", "apiVersion":"v1", "groups":[{
            "name":"apps", "versions":[{"groupVersion":"apps/v1", "version":"v1"}],
            "preferredVersion":{"groupVersion":"apps/v1", "version":"v1"}
        }]}),
        "/apis/apps/v1" => json!({"kind":"APIResourceList", "apiVersion":"v1", "groupVersion":"apps/v1",
            "resources":[{"name":"deployments", "singularName":"deployment", "namespaced":true,
                "kind":"Deployment", "verbs":["get","patch"]}]
        }),
        "/apis/apps/v1/namespaces/example/deployments/example-worker" => deployment(),
        // kubectl 1.37 reads the server version before discovery, to warn about client/server skew.
        "/version" => serde_json::to_value(Info {
            git_version: "v1.37.0".into(),
            major: "1".into(),
            minor: "37".into(),
            ..Info::default()
        })
        .unwrap(),
        _ => panic!("unexpected Kubernetes API path: {path}"),
    }
}

/// Serialize an actual Kubernetes Deployment so discovery and restart tests use the API's real wire schema.
#[expect(
    clippy::single_call_fn,
    reason = "typed deployment data is separate from API discovery routing"
)]
fn deployment() -> Value {
    serde_json::to_value(Deployment {
        metadata: ObjectMeta {
            name: Some("example-worker".into()),
            namespace: Some("example".into()),
            generation: Some(1),
            ..ObjectMeta::default()
        },
        spec: Some(DeploymentSpec {
            replicas: Some(1),
            ..DeploymentSpec::default()
        }),
        status: Some(DeploymentStatus {
            observed_generation: Some(1),
            replicas: Some(1),
            available_replicas: Some(1),
            updated_replicas: Some(1),
            ..DeploymentStatus::default()
        }),
    })
    .unwrap()
}

/// Read a complete request, including the rollout patch body, before returning a close-delimited response.
#[expect(
    clippy::single_call_fn,
    reason = "bounded request parsing is distinct from fixture response selection"
)]
async fn request(stream: &mut TcpStream, diagnostics: Diagnostics) -> String {
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 2048];
    while bytes.len() < 0x0001_0000 {
        let count = stream.read(&mut chunk).await.unwrap();
        assert_ne!(count, 0, "Kubernetes request ended before its body");
        bytes.extend_from_slice(&chunk[..count]);
        diagnostics.record(format_args!("received {count} bytes; request total={}", bytes.len()));
        if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
            let headers = String::from_utf8_lossy(&bytes[..end]);
            // Requests without Content-Length have no body in this fixture.
            let length = headers
                .lines()
                .find_map(|line| {
                    let (key, value) = line.split_once(':')?;
                    key.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().unwrap())
                })
                .unwrap_or_default();
            if bytes.len() >= end.saturating_add(4).saturating_add(length) {
                return String::from_utf8(bytes).unwrap();
            }
        }
    }
    panic!("Kubernetes fixture request exceeded its size bound");
}

/// Reply to discovery, query and patch requests while recording the exact kubectl request paths and bodies.
#[expect(
    clippy::single_call_fn,
    reason = "the asynchronous API fixture must run concurrently with real kubectl"
)]
async fn serve(listener: TcpListener, seen: tokio::sync::mpsc::UnboundedSender<String>, diagnostics: Diagnostics) {
    while !seen.is_closed() {
        diagnostics.record("waiting for a TCP connection");
        let (mut stream, _) = listener.accept().await.unwrap();
        diagnostics.record("accepted a TCP connection; waiting for the complete request");
        let text = tokio::time::timeout(Duration::from_secs(5), request(&mut stream, diagnostics))
            .await
            .unwrap();
        let target = text.split_whitespace().nth(1).unwrap();
        let path = target.split('?').next().unwrap();
        diagnostics.record(format_args!("request complete: {}", text.lines().next().unwrap()));
        let body = response(path).to_string();
        let headers = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n",
            body.len()
        );
        let reply = format!("{headers}Connection: close\r\n\r\n{body}");
        seen.send(text).unwrap();
        stream.write_all(reply.as_bytes()).await.unwrap();
        diagnostics.record("response written; closing the connection");
    }
}

/// Exercise successful command output, readiness decoding and restart acceptance without a real cluster.
#[test]
fn kubectl_queries_and_restarts_only_the_configured_namespace_and_deployment() {
    let _logging = set_default(tracing_subscriber::fmt().with_test_writer().finish());
    let diagnostics = Diagnostics {
        started: Instant::now(),
    };
    let directory = tempfile::tempdir().unwrap();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let listener = runtime
        .block_on(TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)))
        .unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    diagnostics.record(format_args!("API listener bound at {endpoint}; request deadline=5s"));
    let kubeconfig = kubeconfig(directory.path(), &endpoint);
    let cache = directory.path().join("cache");
    temp_env::with_vars(
        [
            ("KUBECONFIG", Some(kubeconfig.as_os_str())),
            ("KUBECACHEDIR", Some(cache.as_os_str())),
        ],
        || {
            runtime.block_on(async {
                let (seen, received) = tokio::sync::mpsc::unbounded_channel();
                let task = tokio::spawn(serve(listener, seen, diagnostics));
                let mut config = et_test_recovery::config();
                config.command_timeout_seconds = 60;
                diagnostics.record(format_args!(
                    "starting kubectl health command; deadline={}s",
                    config.command_timeout_seconds
                ));
                let workload = config.workloads[0].clone();
                let mut backend = Kubernetes::new(config).unwrap();
                assert_eq!(
                    diagnostics.command("health", backend.health(&workload).await, &task),
                    Health::Ready
                );
                diagnostics.record("starting kubectl restart command");
                diagnostics.command("restart", backend.restart(&workload).await, &task);
                assert_restart_request(received);
                task.abort();
                assert!(
                    task.await.unwrap_err().is_cancelled(),
                    "API fixture panicked before teardown"
                );
            });
        },
    );
}

/// Verify that the recorded restart patch targets only the configured namespaced deployment.
#[expect(
    clippy::single_call_fn,
    reason = "request assertions are distinct from command execution and fixture setup"
)]
fn assert_restart_request(mut received: tokio::sync::mpsc::UnboundedReceiver<String>) {
    let mut patch = String::default();
    while let Ok(text) = received.try_recv() {
        if text.starts_with("PATCH ") {
            patch = text;
        }
    }
    assert!(patch.starts_with("PATCH /apis/apps/v1/namespaces/example/deployments/example-worker"));
    assert!(patch.contains("kubectl.kubernetes.io/restartedAt"));
}

/// Keep credentials isolated while making kubeconfig preparation distinct from the timed command exchange.
#[expect(
    clippy::single_call_fn,
    reason = "fixture file preparation is independent of runtime diagnostics"
)]
fn kubeconfig(directory: &Path, endpoint: &str) -> PathBuf {
    let path = directory.join("kubeconfig.json");
    let document = json!({"apiVersion":"v1", "kind":"Config", "current-context":"k3d-example",
        "clusters":[{"name":"fixture", "cluster":{"server":endpoint}}],
        "contexts":[{"name":"k3d-example", "context":{"cluster":"fixture", "user":"fixture"}}],
        "users":[{"name":"fixture", "user":{}}]
    });
    fs::write(&path, serde_json::to_vec(&document).unwrap()).unwrap();
    path
}

/// Verify the diagnostic failure path locally without claiming to reproduce the Windows timeout.
#[tokio::test]
#[should_panic(expected = "fixture_finished=false: diagnostic probe")]
async fn command_failure_reports_the_operation_and_live_fixture() {
    let diagnostics = Diagnostics {
        started: Instant::now(),
    };
    diagnostics.record("fixture waiting for its first TCP connection");
    let task = tokio::spawn(std::future::pending::<()>());
    diagnostics.command::<()>("health", Err(RecoveryError::Invalid("diagnostic probe".into())), &task);
}

/// Preserve target-rejection errors through both public backend methods before launching kubectl.
#[tokio::test]
async fn backend_methods_propagate_non_allowlisted_target_errors() {
    let mut backend = Kubernetes::new(et_test_recovery::config()).unwrap();
    let mut workload = et_test_recovery::config().workloads.remove(0);
    workload.deployment = "not-allowlisted".into();
    let health = backend.health(&workload).await.unwrap_err();
    let restart = backend.restart(&workload).await.unwrap_err();
    assert!(health.to_string().contains("not allowlisted"));
    assert!(restart.to_string().contains("not allowlisted"));
}
