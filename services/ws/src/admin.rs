//! The hub's admin API: blocking addresses and evicting agents, authorized by the bearer token `WS_ADMIN_TOKEN`
//! names.
//!
//! The file-level `#![expect(clippy::exhaustive_structs)]` (active under `openapi-spec`) is scoped here because
//! utoipa's derives emit `pub struct`s without `#[non_exhaustive]`, which no item-level `#[expect]` can reach.

#![cfg_attr(
    feature = "openapi-spec",
    expect(
        clippy::exhaustive_structs,
        reason = "utoipa emits sibling pub structs without #[non_exhaustive]; only file scope can silence the lint"
    )
)]

use std::future::{Ready, ready};
use std::net::IpAddr;

use actix_web::dev::Payload;
use actix_web::http::{StatusCode, header};
use actix_web::{FromRequest, HttpRequest, HttpResponse, ResponseError, web};
use edge_toolkit::ws_server::RegistryError;
use et_hub_backplane::{Backplane, BackplaneError};
use secrecy::{ExposeSecret as _, SecretString};
use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq as _;
use thiserror::Error;
use tracing::{info, warn};

use crate::{WsAgentRegistry, WsConfig, backplane_of};

/// Who may use the admin API, read from the `WS_ADMIN_TOKEN` string.
///
/// Unset, the API is disabled and refuses every request. Set, a request must present the value as an
/// `Authorization: Bearer` credential. An empty value is rejected rather than read as either, since it is more likely
/// a templating slip than a decision.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(try_from = "String")]
#[non_exhaustive]
pub enum AdminToken {
    /// No admin API.
    #[default]
    Disabled,
    /// The bearer token every admin request must present.
    Token(SecretString),
}

/// Why a `WS_ADMIN_TOKEN` value was rejected.
#[derive(Debug, Error)]
#[error("WS_ADMIN_TOKEN is set but empty; unset it to disable the admin API")]
#[non_exhaustive]
pub struct EmptyAdminToken;

impl TryFrom<String> for AdminToken {
    type Error = EmptyAdminToken;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.is_empty() {
            Err(EmptyAdminToken)
        } else {
            Ok(Self::Token(SecretString::from(value)))
        }
    }
}

/// An agent the hub has evicted, and the address now refused because of it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi-spec", derive(utoipa::ToSchema))]
#[non_exhaustive]
pub struct EvictedAgent {
    pub agent_id: String,
    #[cfg_attr(feature = "openapi-spec", schema(value_type = String))]
    pub blocked_ip: IpAddr,
}

/// Everything an admin request can fail with.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum AdminError {
    #[error("the admin API is disabled; set WS_ADMIN_TOKEN to enable it")]
    Disabled,

    #[error("missing or wrong admin bearer token")]
    Unauthorized,

    #[error("unknown agent {0}")]
    UnknownAgent(String),

    #[error("hub backplane unavailable")]
    Backplane(#[source] BackplaneError),

    #[error(transparent)]
    Registry(RegistryError),
}

impl From<RegistryError> for AdminError {
    fn from(error: RegistryError) -> Self {
        if let RegistryError::UnknownAgent(agent_id) = error {
            Self::UnknownAgent(agent_id)
        } else {
            Self::Registry(error)
        }
    }
}

impl From<BackplaneError> for AdminError {
    fn from(error: BackplaneError) -> Self {
        if let BackplaneError::UnknownAgent(agent_id) = error {
            Self::UnknownAgent(agent_id)
        } else {
            Self::Backplane(error)
        }
    }
}

impl ResponseError for AdminError {
    fn status_code(&self) -> StatusCode {
        match self {
            Self::Disabled => StatusCode::FORBIDDEN,
            Self::Unauthorized => StatusCode::UNAUTHORIZED,
            Self::UnknownAgent(_) => StatusCode::NOT_FOUND,
            Self::Backplane(_) => StatusCode::SERVICE_UNAVAILABLE,
            Self::Registry(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    fn error_response(&self) -> HttpResponse {
        let mut response = HttpResponse::build(self.status_code());
        if matches!(self, Self::Unauthorized) {
            let _challenged = response.insert_header((header::WWW_AUTHENTICATE, "Bearer"));
        }
        response.body(self.to_string())
    }
}

/// An admin request that presented the configured token, and the backplane it acts on.
///
/// Taking one as a handler argument is what puts a route behind the admin token: actix refuses the request with the
/// [`AdminError`] before the handler runs.
#[derive(Debug)]
pub struct AdminRequest {
    backplane: Backplane,
}

impl FromRequest for AdminRequest {
    type Error = AdminError;
    type Future = Ready<Result<Self, AdminError>>;

    fn from_request(req: &HttpRequest, _payload: &mut Payload) -> Self::Future {
        let token = req
            .app_data::<web::Data<WsConfig>>()
            .map(|config| config.admin_token.clone())
            .unwrap_or_default();
        ready(authorize(req, &token).map(|()| Self {
            backplane: backplane_of(req),
        }))
    }
}

/// Accept `req` only when it presents the configured token as its bearer credential, logging a refusal.
///
/// The comparison is constant-time in the token's content, so a caller cannot recover it a byte at a time.
#[expect(
    clippy::single_call_fn,
    reason = "the token check of AdminRequest, kept apart from the extractor plumbing around it"
)]
fn authorize(req: &HttpRequest, token: &AdminToken) -> Result<(), AdminError> {
    let AdminToken::Token(expected) = token else {
        warn!("Refused an admin request: {}", AdminError::Disabled);
        return Err(AdminError::Disabled);
    };
    let presented = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    let expected = expected.expose_secret().as_bytes();
    if presented.is_some_and(|credential| bool::from(credential.as_bytes().ct_eq(expected))) {
        Ok(())
    } else {
        warn!("Refused an admin request: {}", AdminError::Unauthorized);
        Err(AdminError::Unauthorized)
    }
}

/// Refuse `ip` on this hub and, through the shared blocklist, on every other hub of `backplane`.
///
/// Every open socket from that address is closed within a heartbeat, whichever agent it carries, and every later
/// connection from it is refused. The block outlives a hub restart: a local hub saves it with its registry, and a
/// NATS-backed hub keeps it in the shared blocklist.
async fn block_everywhere(backplane: &Backplane, registry: &WsAgentRegistry, ip: IpAddr) -> Result<(), AdminError> {
    if let Backplane::Nats(nats) = backplane {
        nats.block_ip(ip).await?;
    }
    let _newly = registry.blocklist.block(ip);
    Ok(())
}

/// Block an address: refuse it on every hub of the backplane.
///
/// Blocking an address already blocked succeeds again, so the request can be repeated safely. A path segment that
/// is not an IPv4 or IPv6 address matches no route and is answered 404.
#[cfg_attr(
    feature = "openapi-spec",
    utoipa::path(
        put,
        path = "/blocked-ips/{ip}",
        tag = "admin",
        params(("ip" = String, Path, description = "IPv4 or IPv6 address to block")),
        security(("admin_token" = [])),
        responses(
            (status = 204, description = "Address blocked"),
            (status = 401, description = "Missing or wrong admin bearer token"),
            (status = 403, description = "Admin API disabled"),
            (status = 404, description = "Not an IP address"),
            (status = 503, description = "Hub backplane unavailable")
        )
    )
)]
pub async fn block_ip(
    admin: AdminRequest,
    ip: web::Path<IpAddr>,
    registry: web::Data<WsAgentRegistry>,
) -> Result<HttpResponse, AdminError> {
    let ip = ip.into_inner();
    block_everywhere(&admin.backplane, &registry, ip).await?;
    info!("Blocked IP {ip}");
    Ok(HttpResponse::NoContent().finish())
}

/// Evict an agent: block the address it last connected from, and drop the messages it queued.
///
/// The block is the one [`block_ip`] makes, so it takes every agent on that address with it. The direct messages
/// the agent sent that are still waiting for their recipients are dropped, so nothing it queued is delivered after
/// it is gone.
#[cfg_attr(
    feature = "openapi-spec",
    utoipa::path(
        delete,
        path = "/agents/{agent_id}",
        tag = "admin",
        params(("agent_id" = String, Path, description = "Agent to evict")),
        security(("admin_token" = [])),
        responses(
            (status = 200, description = "Agent evicted and its address blocked", body = EvictedAgent),
            (status = 401, description = "Missing or wrong admin bearer token"),
            (status = 403, description = "Admin API disabled"),
            (status = 404, description = "Agent not found"),
            (status = 503, description = "Hub backplane unavailable")
        )
    )
)]
pub async fn evict_agent(
    admin: AdminRequest,
    agent_id: web::Path<String>,
    registry: web::Data<WsAgentRegistry>,
) -> Result<HttpResponse, AdminError> {
    let agent_id = agent_id.into_inner();
    let backplane = admin.backplane;
    let blocked_ip = match &backplane {
        Backplane::Local => registry.agent_ip(&agent_id)?,
        Backplane::Nats(nats) => nats.agent_ip(&agent_id).await?,
    };
    block_everywhere(&backplane, &registry, blocked_ip).await?;
    let dropped = match &backplane {
        Backplane::Local => registry.forget_messages_from(&agent_id),
        Backplane::Nats(nats) => nats.forget_messages_from(&agent_id).await?,
    };
    info!("Evicted agent {agent_id}; blocked IP {blocked_ip}; dropped {dropped} queued messages it sent");
    Ok(HttpResponse::Ok().json(EvictedAgent { agent_id, blocked_ip }))
}
