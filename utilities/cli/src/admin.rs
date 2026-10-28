//! `et-cli block-ip` and `et-cli evict-agent`: ask a running hub to block an address or evict an agent through its
//! admin API.
//!
//! The request is made with `reqwest` rather than the generated `et-rest-client`, because et-int-gen, which generates
//! that client, builds this crate to write its HELP.md: a route added to the hub and used here would otherwise need
//! the client regenerated before et-int-gen could build to regenerate it.

use std::fmt;
use std::net::IpAddr;

use reqwest::{Method, Response};
use serde::Deserialize;

use crate::CliError;

/// What the hub reports for an eviction: the agent, and the address it now refuses.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[non_exhaustive]
pub struct EvictedAgent {
    pub agent_id: String,
    pub blocked_ip: IpAddr,
}

impl fmt::Display for EvictedAgent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Evicted agent {}; blocked IP {}", self.agent_id, self.blocked_ip)
    }
}

/// Block `ip` on the hub at `hub_url` and every hub sharing its backplane, authorizing with its `admin_token`.
pub fn block_ip(hub_url: &str, ip: IpAddr, admin_token: &str) -> Result<(), CliError> {
    let _body = call_hub(
        Method::PUT,
        hub_url,
        &format!("/blocked-ips/{ip}"),
        admin_token,
        Response::bytes,
    )?;
    Ok(())
}

/// Evict `agent_id` from the hub at `hub_url`, authorizing with that hub's `admin_token`.
///
/// The hub blocks the address the agent last connected from, on every hub sharing its backplane, and drops the
/// messages the agent queued.
pub fn evict_agent(hub_url: &str, agent_id: &str, admin_token: &str) -> Result<EvictedAgent, CliError> {
    call_hub(
        Method::DELETE,
        hub_url,
        &format!("/agents/{agent_id}"),
        admin_token,
        Response::json,
    )
}

/// Send `method` to `path` on the hub at `hub_url` with its `admin_token`, and `read` the successful response.
fn call_hub<T, F>(
    method: Method,
    hub_url: &str,
    path: &str,
    admin_token: &str,
    read: impl FnOnce(Response) -> F,
) -> Result<T, CliError>
where
    F: Future<Output = reqwest::Result<T>>,
{
    let url = format!("{}{path}", hub_url.trim_end_matches('/'));
    let request = reqwest::Client::new().request(method, url).bearer_auth(admin_token);
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    Ok(runtime.block_on(async { read(request.send().await?.error_for_status()?).await })?)
}
