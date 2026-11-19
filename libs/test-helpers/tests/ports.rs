//! Covers `free_listener`: a loopback port that is served from the moment it is handed out, and held until dropped.
#![cfg(test)]

use std::net::{Ipv4Addr, TcpStream};

use et_test_helpers::{free_listener, wait_for_port};
use pretty_assertions::assert_eq;
use pretty_assertions::assert_ne;

#[test]
fn a_free_listener_is_on_loopback_and_takes_a_connection_at_once() {
    let listener = free_listener();
    let address = listener.local_addr().unwrap();
    assert_eq!(address.ip(), Ipv4Addr::LOCALHOST);
    assert_ne!(address.port(), 0);
    let _client = TcpStream::connect(address).unwrap();
    let (_served, peer) = listener.accept().unwrap();
    assert!(peer.ip().is_loopback());
}

#[test]
fn a_held_listener_keeps_its_port_from_the_next_one() {
    let first = free_listener();
    let second = free_listener();
    assert_ne!(first.local_addr().unwrap().port(), second.local_addr().unwrap().port());
}

#[test]
fn a_free_listener_is_reachable_before_anything_accepts() {
    let listener = free_listener();
    assert!(wait_for_port(listener.local_addr().unwrap().port()));
}
