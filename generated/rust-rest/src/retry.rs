//! Native retry policy for replayable, idempotent REST requests.
//!
//! Kept separate from the generated operations so regeneration cannot discard the policy. An SDK call includes
//! retry/backoff time; each retry is logged without request bodies, URLs or credentials. There are at most 32
//! attempts and no retry starts beyond a 90-second window. The first attempt retains the caller's HTTP timeout;
//! later attempts also stop at the retry deadline. The policy does not replay a response body that fails after
//! response headers have already been returned. Numeric Retry-After delays are honoured; unsupported values stop
//! retry rather than risking an earlier request than the server permits. Browser WASM keeps its original policy.

use std::time::{Duration, SystemTime};

use reqwest::{Client, Method, Request, Response};
use retry_policies::policies::{ExponentialBackoff, ExponentialBackoffTimed};
use retry_policies::{RetryDecision, RetryPolicy as _};
use tokio::time::Instant;

struct Limits {
    attempts: u32,
    window: Duration,
    minimum_backoff: Duration,
    maximum_backoff: Duration,
}

enum Delay {
    Wait(Duration),
    Stop,
}

/// A streaming body cannot be copied for another request attempt.
#[derive(Debug)]
struct NonReplayableBody;

/// Clone a replayable request without manufacturing a transport error for a non-replayable body.
fn clone_request(request: &Request) -> Result<Request, NonReplayableBody> {
    request.try_clone().ok_or(NonReplayableBody)
}

/// Retry temporary request failures while preserving the final HTTP/error outcome on exhaustion.
pub(crate) async fn execute(client: &Client, request: Request) -> reqwest::Result<Response> {
    run(
        client,
        request,
        Limits {
            attempts: 32,
            window: Duration::from_secs(90),
            minimum_backoff: Duration::from_millis(250),
            maximum_backoff: Duration::from_secs(5),
        },
    )
    .await
}

async fn run(client: &Client, request: Request, limits: Limits) -> reqwest::Result<Response> {
    if !matches!(
        *request.method(),
        Method::GET | Method::HEAD | Method::PUT | Method::DELETE | Method::OPTIONS
    ) {
        return client.execute(request).await;
    }
    // Streaming bodies cannot be replayed. The original request must still be sent exactly once.
    let Ok(first) = clone_request(&request) else {
        return client.execute(request).await;
    };
    let deadline = Instant::now() + limits.window;
    let started = SystemTime::now();
    let policy = ExponentialBackoff::builder()
        .retry_bounds(limits.minimum_backoff, limits.maximum_backoff)
        .build_with_total_retry_duration(limits.window);
    // The first attempt retains the caller's HTTP timeout. Subsequent attempts are also bounded by the window.
    let mut outcome = client.execute(first).await;
    let mut attempts = 1;
    while attempts < limits.attempts && retryable(&outcome) {
        let Delay::Wait(wait) = next_delay(&policy, started, attempts, &outcome, deadline) else {
            break;
        };
        let method = request.method();
        let http_status = outcome.as_ref().map_or(0, |response| response.status().as_u16());
        let retry_after_seconds = wait.as_secs_f64();
        tracing::warn!(
            method = %method,
            http_status,
            attempt = attempts,
            retry_after_seconds,
            "retrying replayable REST request"
        );
        tokio::time::sleep(wait).await;
        if Instant::now() >= deadline {
            break;
        }
        let Ok(next) = clone_request(&request) else {
            break;
        };
        attempts += 1;
        match tokio::time::timeout_at(deadline, client.execute(next)).await {
            Ok(result) => outcome = result,
            // Retain the previous concrete failure rather than inventing a successful/empty response.
            Err(_) => break,
        }
    }
    outcome
}

/// Combine policy backoff and Retry-After without scheduling an attempt beyond the retry window.
fn next_delay(
    policy: &ExponentialBackoffTimed,
    started: SystemTime,
    attempts: u32,
    outcome: &reqwest::Result<Response>,
    deadline: Instant,
) -> Delay {
    let RetryDecision::Retry { execute_after } = policy.should_retry(started, attempts - 1) else {
        return Delay::Stop;
    };
    let fallback = execute_after.duration_since(SystemTime::now()).unwrap_or_default();
    let Delay::Wait(wait) = retry_delay(outcome, fallback) else {
        return Delay::Stop;
    };
    if wait >= deadline.saturating_duration_since(Instant::now()) {
        return Delay::Stop;
    }
    Delay::Wait(wait)
}

fn retryable(outcome: &reqwest::Result<Response>) -> bool {
    match outcome {
        Ok(response) => matches!(response.status().as_u16(), 408 | 429 | 500 | 502 | 503 | 504),
        Err(error) => error.is_timeout() || error.is_connect() || error.is_request() || error.is_body(),
    }
}

fn retry_delay(outcome: &reqwest::Result<Response>, fallback: Duration) -> Delay {
    let Ok(response) = outcome else {
        return Delay::Wait(fallback);
    };
    // An absent Retry-After header is normal. An unsupported/malformed value conservatively prevents retry.
    let Some(value) = response.headers().get(reqwest::header::RETRY_AFTER) else {
        return Delay::Wait(fallback);
    };
    let Ok(text) = value.to_str() else {
        return Delay::Stop;
    };
    let Ok(seconds) = text.parse::<u64>() else {
        return Delay::Stop;
    };
    Delay::Wait(fallback.max(Duration::from_secs(seconds)))
}

#[cfg(test)]
// The compiler resolves this test module against this source file, independently of runtime working directories.
// nosemgrep: no-semgrep-mentions, no-ast-grep-mentions -- Required compile-time path annotation.
// ast-grep-ignore: no-relative-path-literal
#[path = "../tests/unit/retry.rs"]
mod tests;
