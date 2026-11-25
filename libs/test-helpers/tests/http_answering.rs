//! Covers `http_answering`: every request gets the chosen status, and each one is counted by the time it is answered.
#![cfg(test)]

use std::io::{Read as _, Write as _};
use std::net::TcpStream;
use std::sync::atomic::Ordering;

use et_test_helpers::http_answering;
use pretty_assertions::assert_eq;

/// The status line answering one `GET` sent to `port`.
fn status_line(port: u16) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream.write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n").unwrap();
    let mut response = String::default();
    let _read = stream.read_to_string(&mut response).unwrap();
    response.lines().next().unwrap().to_owned()
}

#[test]
fn every_request_is_answered_with_the_status_and_counted() {
    let (port, requests) = http_answering("418 I'm a teapot");

    assert_eq!(status_line(port), "HTTP/1.1 418 I'm a teapot");
    assert_eq!(status_line(port), "HTTP/1.1 418 I'm a teapot");
    assert_eq!(requests.load(Ordering::SeqCst), 2);
}
