//! Which backplane a hub routes through.

use std::fmt;
use std::str::FromStr;

use serde::Deserialize;

use crate::BackplaneError;

/// Which backplane a hub routes through, read from the `WS_BACKPLANE` string.
///
/// `local` keeps every agent the hub knows inside this one process. A `nats://` or `tls://` URL names a NATS server
/// with `JetStream`, and every hub pointed at the same one shares a single agent directory, mailbox and delivery
/// fabric, so agents attached to different hubs reach each other as if they shared one.
///
/// The URL may carry credentials, so its `Debug` form, and every error that names it, shows `***` in their place.
#[derive(Clone, Default, Deserialize, Eq, PartialEq)]
#[serde(try_from = "String")]
#[non_exhaustive]
pub enum BackplaneConfig {
    /// This process alone.
    #[default]
    Local,
    /// The NATS server at this URL.
    Nats(String),
}

impl fmt::Debug for BackplaneConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Local => f.write_str("Local"),
            Self::Nats(url) => f.debug_tuple("Nats").field(&redact_userinfo(url)).finish(),
        }
    }
}

impl FromStr for BackplaneConfig {
    type Err = BackplaneError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value == "local" {
            Ok(Self::Local)
        } else if value.starts_with("nats://") || value.starts_with("tls://") {
            Ok(Self::Nats(value.to_owned()))
        } else {
            Err(BackplaneError::UnrecognisedConfig(redact_userinfo(value)))
        }
    }
}

impl TryFrom<String> for BackplaneConfig {
    type Error = BackplaneError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.parse()
    }
}

/// `urls` -- one URL, or several separated by commas as NATS accepts -- with each `user:password@` shown as `***@`.
///
/// Scheme, host, port and path are kept, so the result still says which server was meant.
pub(crate) fn redact_userinfo(urls: &str) -> String {
    urls.split(',').map(redact_one).collect::<Vec<_>>().join(",")
}

/// One URL with its userinfo, if it has any, shown as `***`; a value with no scheme is read as starting at its host.
#[expect(
    clippy::single_call_fn,
    reason = "the per-URL step of redact_userinfo, mapped over a comma-separated list"
)]
fn redact_one(url: &str) -> String {
    let (scheme, rest) = url.split_once("://").unwrap_or(("", url));
    let (authority, path) = rest.find('/').map_or((rest, ""), |slash| rest.split_at(slash));
    let Some((_userinfo, host)) = authority.rsplit_once('@') else {
        return url.to_owned();
    };
    let separator = if scheme.is_empty() { "" } else { "://" };
    format!("{scheme}{separator}***@{host}{path}")
}
