#![cfg(test)]

use edge_toolkit::ports::Services;

#[rstest::rstest]
#[case::otlp_collector(Services::OtlpCollector, 5080)]
#[case::insecure_web_socket_server(Services::InsecureWebSocketServer, 8080)]
#[case::secure_web_socket_server(Services::SecureWebSocketServer, 8443)]
#[case::backplane(Services::Backplane, 4222)]
#[case::backplane_monitor(Services::BackplaneMonitor, 8222)]
#[case::shared_storage(Services::SharedStorage, 9000)]
fn port(#[case] service: Services, #[case] expected: u16) {
    assert_eq!(service.port(), expected);
}
