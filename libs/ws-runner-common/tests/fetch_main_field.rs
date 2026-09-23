//! Covers `fetch_main_field` against a hub that does not serve the module straight away.
//!
//! The retry exists for the race a deployment always has: the hub binds its port before it has finished
//! scanning the module paths, so a runner starting alongside it can be answered 404 for a module that is about
//! to appear. The stub here reproduces exactly that -- a listener that refuses the module a few times and then
//! serves it -- because the real hub only loses the race under load, which would make this test a coin toss.
#![cfg(test)]

use std::io::{Read as _, Write as _};
use std::net::TcpListener;
use std::thread::{self, JoinHandle};

use et_test_helpers::reserve_port;
use et_ws_runner_common::{BootstrapError, fetch_main_field};

/// The `package.json` the stub hub serves once it admits to having the module.
const PACKAGE_JSON: &str = r#"{"main":"index.js"}"#;

/// Serve `misses` 404s on `port`, then one 200 carrying `body`, then stop.
///
/// Deliberately not an HTTP library: the point is to choose the status of each individual response, and a
/// handful of bytes down a `TcpStream` does that without pulling a server framework into a low-level test.
fn spawn_hub(port: u16, misses: usize, body: &'static str) -> JoinHandle<()> {
    let listener = TcpListener::bind(("127.0.0.1", port)).unwrap();
    thread::spawn(move || {
        let mut answered = 0_usize;
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            // Read only so the client can finish sending; nothing here inspects the request.
            let mut request = [0_u8; 1024];
            let _read = stream.read(&mut request);
            let response = if answered < misses {
                "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string()
            } else {
                let length = body.len();
                let head = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {length}");
                format!("{head}\r\nConnection: close\r\n\r\n{body}")
            };
            let _written = stream.write_all(response.as_bytes());
            let _flushed = stream.flush();
            answered = answered.saturating_add(1);
            if answered > misses {
                break;
            }
        }
    })
}

/// Point a REST client at the stub hub on `port`.
fn client_for(port: u16) -> et_rest_client::Client {
    et_rest_client::Client::new(&format!("http://127.0.0.1:{port}"))
}

/// Fetch the module's `main` from a hub that answers `misses` 404s before serving it.
async fn main_field_after(misses: usize) -> String {
    let port = reserve_port();
    let hub = spawn_hub(port, misses, PACKAGE_JSON);

    let main = fetch_main_field(&client_for(port), "et-ws-math1").await.unwrap();

    hub.join().unwrap();
    main
}

#[tokio::test]
async fn fetch_main_field_serves_a_module_that_is_ready_immediately() {
    assert_eq!(main_field_after(0).await, "index.js");
}

#[tokio::test]
async fn fetch_main_field_retries_until_the_hub_serves_the_module() {
    assert_eq!(main_field_after(2).await, "index.js");
}

#[tokio::test(start_paused = true)]
async fn fetch_main_field_gives_up_once_the_wait_window_is_spent() {
    let port = reserve_port();
    // A hub that never admits to having the module: the count of 404s to serve is larger than the window can
    // ask for, so every attempt misses and the loop runs until it is out of time. That is a genuinely absent
    // module rather than a slow one, and it has to be reported instead of retried forever -- a runner stuck
    // here looks identical to one that is working.
    //
    // Paused time is what makes this affordable. The window is two minutes of retries half a second apart;
    // the sleeps between attempts advance the clock instantly, so the test spends real time only on the
    // request each attempt makes.
    //
    // The handle is dropped rather than joined: this hub never stops answering, so there is nothing to wait
    // for. Each test runs in its own process, so the listener goes away with it.
    let _hub = spawn_hub(port, usize::MAX, PACKAGE_JSON);

    let error = fetch_main_field(&client_for(port), "et-ws-never-served")
        .await
        .unwrap_err();

    // `Stream` rather than `Rest`: the fetch goes through the raw client, because the typed one encodes its
    // arguments as single path segments and a module name carries the scope it publishes under.
    assert!(
        matches!(&error, BootstrapError::Stream(_)),
        "expected the last fetch failure to be reported, got: {error:?}"
    );
}

#[tokio::test]
async fn fetch_main_field_rejects_a_package_json_without_a_main_field() {
    let port = reserve_port();
    // A body that parses but names no entry point is a property of the module rather than of timing, so it
    // must fail on the first attempt instead of being retried for the whole window.
    let hub = spawn_hub(port, 0, r#"{"name":"et-ws-math1"}"#);

    let error = fetch_main_field(&client_for(port), "et-ws-math1").await.unwrap_err();

    assert!(
        matches!(&error, BootstrapError::PackageJsonMissingMain { module } if module == "et-ws-math1"),
        "expected PackageJsonMissingMain, got: {error:?}"
    );
    hub.join().unwrap();
}
