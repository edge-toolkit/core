//! Retry policy and HTTP request-capture fixtures preserve payloads and failure outcomes.
#![cfg(test)]

use std::collections::BTreeMap;
use std::error::Error;
use std::io::{Seek as _, Write as _};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};
use tracing::instrument::WithSubscriber as _;
use tracing_subscriber::layer::SubscriberExt as _;

use super::{Limits, clone_request, run};

type TestError = Box<dyn Error + Send + Sync>;
type ServerTask = tokio::task::JoinHandle<Result<Vec<Vec<u8>>, TestError>>;

/// Create a genuine non-replayable file body without depending on source paths or leaving files behind.
fn streamed_body(bytes: &[u8]) -> Result<reqwest::Body, TestError> {
    let mut file = tempfile::tempfile()?;
    file.write_all(bytes)?;
    file.rewind()?;
    Ok(tokio::fs::File::from_std(file).into())
}

/// Capture retry diagnostics and signal exactly when the request enters its backoff wait.
#[derive(Clone, Default)]
struct RetryLog {
    events: Arc<Mutex<Vec<BTreeMap<String, String>>>>,
    waiting: Arc<tokio::sync::Notify>,
}

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for RetryLog {
    fn on_event(&self, event: &tracing::Event<'_>, _context: tracing_subscriber::layer::Context<'_, S>) {
        if event.metadata().level() != &tracing::Level::WARN || !event.metadata().target().ends_with("::retry") {
            return;
        }
        let mut fields = BTreeMap::new();
        event.record(&mut |field: &tracing::field::Field, value: &dyn std::fmt::Debug| {
            fields.insert(field.name().to_owned(), format!("{value:?}"));
        });
        self.events.lock().expect("retry log mutex poisoned").push(fields);
        self.waiting.notify_one();
    }
}

/// Retry diagnostics identify the operation and delay without exposing its URL, credentials or payload.
#[tokio::test]
async fn retry_logs_safe_operation_fields() -> Result<(), TestError> {
    let (url, task) = server(vec!["408 Request Timeout", "200 OK"], "").await?;
    let client = reqwest::Client::new();
    let request = client
        .put(format!("{url}/private-object?token=secret"))
        .body("private payload")
        .build()?;
    let log = RetryLog::default();
    let response = run(&client, request, limits(4))
        .with_subscriber(tracing_subscriber::registry().with(log.clone()))
        .await?;
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    assert_eq!(task.await??.len(), 2);
    let events = log.events.lock().expect("retry log mutex poisoned");
    assert_eq!(events.len(), 1);
    let fields = &events[0];
    assert_eq!(fields["method"], "PUT");
    assert_eq!(fields["http_status"], "408");
    assert_eq!(fields["attempt"], "1");
    assert!(fields["retry_after_seconds"].parse::<f64>()? >= 0.0);
    assert!(fields["message"].contains("retrying replayable REST request"));
    assert_eq!(fields.len(), 5);
    assert!(!format!("{fields:?}").contains("private"));
    assert!(!format!("{fields:?}").contains("secret"));
    Ok(())
}

/// A task resumed after its backoff deadline returns the original failure without sending another request.
#[tokio::test]
async fn deadline_expiring_during_backoff_prevents_replay() -> Result<(), TestError> {
    let (url, task) = server(vec!["408 Request Timeout"], "").await?;
    let client = reqwest::Client::new();
    let log = RetryLog::default();
    let waiting = Arc::clone(&log.waiting);
    let clock = tokio::spawn(async move {
        waiting.notified().await;
        // Advance only after the real HTTP response schedules a retry, avoiding simulated network timeouts.
        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(3)).await;
    });
    let response = run(&client, client.get(url).build()?, limits(4))
        .with_subscriber(tracing_subscriber::registry().with(log.clone()))
        .await?;
    clock.await?;
    assert_eq!(response.status(), reqwest::StatusCode::REQUEST_TIMEOUT);
    assert_eq!(task.await??.len(), 1);
    assert_eq!(log.events.lock().expect("retry log mutex poisoned").len(), 1);
    Ok(())
}

fn limits(attempts: u32) -> Limits {
    Limits {
        attempts,
        window: Duration::from_secs(2),
        minimum_backoff: Duration::from_millis(1),
        maximum_backoff: Duration::from_millis(2),
    }
}

async fn server(statuses: Vec<&'static str>, header: &'static str) -> Result<(String, ServerTask), TestError> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let url = format!("http://{}", listener.local_addr()?);
    let task = tokio::spawn(async move {
        let mut bodies = Vec::new();
        for status in statuses {
            let (mut socket, _) = listener.accept().await?;
            bodies.push(request_body(&mut socket).await?);
            if status == "DROP" {
                continue;
            }
            let status = if status == "SLOW" {
                tokio::time::sleep(Duration::from_millis(25)).await;
                "503 Service Unavailable"
            } else {
                status
            };
            if status == "HANG" {
                tokio::time::sleep(Duration::from_secs(60)).await;
                break;
            }
            socket
                .write_all(
                    format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n{header}\r\n").as_bytes(),
                )
                .await?;
        }
        Ok(bodies)
    });
    Ok((url, task))
}

/// Read one complete buffered HTTP request so tests can compare the payload across retries.
async fn request_body(socket: &mut TcpStream) -> Result<Vec<u8>, TestError> {
    let mut input = Vec::new();
    let mut buffer = [0; 4096];
    let boundary = loop {
        let read = socket.read(&mut buffer).await?;
        if read == 0 {
            return Err("request ended before headers".into());
        }
        input.extend_from_slice(&buffer[..read]);
        if let Some(index) = input.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
            break index + 4;
        }
    };
    let headers = String::from_utf8_lossy(&input[..boundary]).to_ascii_lowercase();
    let length = headers
        .lines()
        .find_map(|line| line.strip_prefix("content-length:"))
        .unwrap_or("0")
        .trim()
        .parse::<usize>()?;
    while input.len() < boundary + length {
        let read = socket.read(&mut buffer).await?;
        if read == 0 {
            return Err("request ended before body".into());
        }
        input.extend_from_slice(&buffer[..read]);
    }
    Ok(input[boundary..boundary + length].to_vec())
}

/// The fixture must reject truncated uploads rather than report a successful payload comparison.
#[tokio::test]
async fn request_capture_rejects_incomplete_or_invalid_requests() -> Result<(), TestError> {
    for (request, expected) in [
        (b"PUT / HTTP/1.1\r\nContent-Length: 4\r\n".as_slice(), "before headers"),
        (
            b"PUT / HTTP/1.1\r\nContent-Length: 4\r\n\r\nab".as_slice(),
            "before body",
        ),
        (
            b"PUT / HTTP/1.1\r\nContent-Length: bad\r\n\r\n".as_slice(),
            "invalid digit",
        ),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let capture = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await?;
            request_body(&mut socket).await
        });
        let mut client = TcpStream::connect(address).await?;
        client.write_all(request).await?;
        client.shutdown().await?;
        let error = tokio::time::timeout(Duration::from_secs(2), capture)
            .await??
            .unwrap_err();
        assert!(error.to_string().contains(expected), "{error}");
    }
    Ok(())
}

/// A checkpoint upload spanning several socket reads is replayed without losing or duplicating bytes.
#[tokio::test]
async fn buffered_put_replays_identical_bytes_after_http_408() -> Result<(), TestError> {
    let (url, task) = server(vec!["408 Request Timeout", "200 OK"], "").await?;
    let client = reqwest::Client::new();
    let payload = vec![0x5a; 64 * 1024];
    let request = client.put(url).body(payload.clone()).build()?;
    let response = run(&client, request, limits(4)).await?;
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    assert_eq!(task.await??, vec![payload.clone(), payload]);
    Ok(())
}

/// A non-replayable file stream is sent once and preserves HTTP 408 without attempting to reuse its consumed body.
#[tokio::test]
async fn streaming_put_is_sent_once_without_retrying_http_408() -> Result<(), TestError> {
    let expected = b"a file-backed update must be sent exactly once";
    let (url, task) = server(vec!["408 Request Timeout"], "").await?;
    let client = reqwest::Client::new();
    let request = client
        .put(url)
        .header(reqwest::header::CONTENT_LENGTH, expected.len().to_string())
        .body(streamed_body(expected)?)
        .build()?;
    assert!(
        request.try_clone().is_none(),
        "the fixture must exercise a non-replayable body"
    );
    let response = run(&client, request, limits(4)).await?;
    assert_eq!(response.status(), reqwest::StatusCode::REQUEST_TIMEOUT);
    assert_eq!(task.await??, vec![expected.to_vec()]);
    Ok(())
}

/// Cloning retains a buffered operation's identity and payload, while a real file stream is explicitly rejected.
#[tokio::test]
async fn request_clone_preserves_buffered_requests_and_rejects_streams() -> Result<(), TestError> {
    let client = reqwest::Client::new();
    let buffered = client
        .put("http://127.0.0.1/immutable-update")
        .header("x-update-id", "same-update")
        .body("immutable payload")
        .build()?;
    let cloned = clone_request(&buffered).expect("buffered bodies must remain replayable");
    assert_eq!(cloned.method(), buffered.method());
    assert_eq!(cloned.url(), buffered.url());
    assert_eq!(cloned.headers(), buffered.headers());
    assert_eq!(cloned.body().unwrap().as_bytes(), Some(b"immutable payload".as_slice()));
    let streamed = client
        .put(buffered.url().clone())
        .body(streamed_body(b"streamed update")?)
        .build()?;
    assert!(clone_request(&streamed).is_err());
    Ok(())
}

#[tokio::test]
async fn permanent_error_and_post_are_not_retried() -> Result<(), TestError> {
    for (method, status) in [
        (reqwest::Method::PUT, "403 Forbidden"),
        (reqwest::Method::POST, "503 Service Unavailable"),
    ] {
        let (url, task) = server(vec![status], "").await?;
        let client = reqwest::Client::new();
        let request = client.request(method, url).body("one operation").build()?;
        let response = run(&client, request, limits(4)).await?;
        assert!(response.status().is_client_error() || response.status().is_server_error());
        assert_eq!(task.await??.len(), 1);
    }
    Ok(())
}

#[tokio::test]
async fn exhaustion_returns_last_http_failure() -> Result<(), TestError> {
    let (url, task) = server(vec!["503 Service Unavailable"; 3], "").await?;
    let client = reqwest::Client::new();
    let response = run(&client, client.get(url).build()?, limits(3)).await?;
    assert_eq!(response.status(), reqwest::StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(task.await??.len(), 3);
    Ok(())
}

#[tokio::test]
async fn retry_after_beyond_budget_preserves_response() -> Result<(), TestError> {
    let (url, task) = server(vec!["429 Too Many Requests"], "Retry-After: 60\r\n").await?;
    let client = reqwest::Client::new();
    let response = run(&client, client.get(url).build()?, limits(4)).await?;
    assert_eq!(response.status(), reqwest::StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(task.await??.len(), 1);
    Ok(())
}

/// A lost response logs the absent HTTP status and retries the same buffered update without changing its payload.
#[tokio::test]
async fn a_connection_closed_before_response_retries_the_same_payload() -> Result<(), TestError> {
    let (url, task) = server(vec!["DROP", "200 OK"], "").await?;
    let client = reqwest::Client::new();
    let payload = b"immutable update after a transport error".to_vec();
    let request = client.put(url).body(payload.clone()).build()?;
    let log = RetryLog::default();
    let response = run(&client, request, limits(4))
        .with_subscriber(tracing_subscriber::registry().with(log.clone()))
        .await?;
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    assert_eq!(task.await??, vec![payload.clone(), payload]);
    let events = log.events.lock().expect("retry log mutex poisoned");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["http_status"], "0");
    assert_eq!(events[0]["method"], "PUT");
    Ok(())
}

/// Unsupported server delays stop replay, including non-ASCII values and integers outside the supported range.
#[tokio::test]
async fn malformed_retry_after_headers_preserve_the_response_without_replay() -> Result<(), TestError> {
    for header in [
        "Retry-After: tomorrow\r\n",
        "Retry-After: \u{e9}\r\n",
        "Retry-After: 18446744073709551616\r\n",
    ] {
        let (url, task) = server(vec!["503 Service Unavailable"], header).await?;
        let client = reqwest::Client::new();
        let response = run(&client, client.get(url).build()?, limits(4)).await?;
        assert_eq!(response.status(), reqwest::StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(task.await??.len(), 1);
    }
    Ok(())
}

/// The first request retains its own timeout, but a slow failure cannot start retries after the budget expires.
#[tokio::test]
async fn a_slow_first_attempt_does_not_extend_the_retry_window() -> Result<(), TestError> {
    let (url, task) = server(vec!["SLOW"], "").await?;
    let client = reqwest::Client::builder().timeout(Duration::from_secs(2)).build()?;
    let budget = Limits {
        window: Duration::from_millis(1),
        ..limits(4)
    };
    let response = run(&client, client.get(url).build()?, budget).await?;
    assert_eq!(response.status(), reqwest::StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(task.await??.len(), 1);
    Ok(())
}

#[tokio::test]
async fn generated_storage_operation_uses_the_shared_policy() -> Result<(), TestError> {
    let (url, task) = server(vec!["408 Request Timeout", "200 OK"], "").await?;
    let client = crate::Client::new(&url);
    let payload = b"same immutable update".to_vec();
    client.put_file("agent", "update.pt", payload.clone()).await?;
    assert_eq!(task.await??, vec![payload.clone(), payload]);
    Ok(())
}

#[tokio::test]
async fn a_stalled_retry_stops_at_the_window_and_preserves_failure() -> Result<(), TestError> {
    let (url, task) = server(vec!["408 Request Timeout", "HANG"], "").await?;
    let client = reqwest::Client::new();
    let budget = Limits {
        window: Duration::from_millis(200),
        ..limits(4)
    };
    let response =
        tokio::time::timeout(Duration::from_secs(2), run(&client, client.get(url).build()?, budget)).await??;
    assert_eq!(response.status(), reqwest::StatusCode::REQUEST_TIMEOUT);
    task.abort();
    let _result = task.await;
    Ok(())
}
