//! Covers the agent summaries `server_message_to_wit` hands a guest: each address family crosses the WIT boundary
//! as its own fixed-size `ip-address` case, with its octets or hextets intact and in order.
#![cfg(test)]

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use edge_toolkit::ws::{AgentConnectionState, AgentSummary, ServerMessage};
use et_ws_wasi_runner::bindings::et::ws_messages::messages::{IpAddress, ServerMessage as WitServerMessage};
use et_ws_wasi_runner::host::ws::server_message_to_wit;

#[test]
fn list_agents_carries_each_address_family_as_its_own_case() {
    let agents = vec![
        AgentSummary::new(
            "agent-v4".to_owned(),
            AgentConnectionState::Connected,
            IpAddr::V4(Ipv4Addr::new(192, 0, 2, 7)),
        ),
        AgentSummary::new(
            "agent-v6".to_owned(),
            AgentConnectionState::Disconnected,
            IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 6)),
        ),
    ];

    let WitServerMessage::ListAgentsResponse(payload) =
        server_message_to_wit(ServerMessage::ListAgentsResponse { agents }).unwrap()
    else {
        panic!("a list-agents response must stay a list-agents response");
    };
    let addresses: Vec<IpAddress> = payload.agents.iter().map(|agent| agent.last_known_ip).collect();

    assert!(matches!(
        addresses.as_slice(),
        [
            IpAddress::Ipv4((192, 0, 2, 7)),
            IpAddress::Ipv6((0x2001, 0xdb8, 0, 0, 0, 0, 0, 6))
        ]
    ));
}
