//! Everything a backplane call can fail with.

use std::string::FromUtf8Error;

use thiserror::Error;

use crate::config::redact_userinfo;

/// The URL a variant carries has had its credentials replaced by `***` before it was stored.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum BackplaneError {
    #[error("backplane {0:?} is neither `local` nor a nats:// or tls:// URL")]
    UnrecognisedConfig(String),

    #[error("failed to connect to NATS at {url}")]
    Connect {
        url: String,
        #[source]
        source: async_nats::ConnectError,
    },

    #[error("NATS failed to {action}")]
    Nats {
        action: &'static str,
        #[source]
        source: async_nats::Error,
    },

    #[error("a backplane record is not valid JSON")]
    Codec(#[from] serde_json::Error),

    #[error("a backplane key is not hex-encoded UTF-8")]
    Key(#[from] const_hex::FromHexError),

    #[error("a backplane message lacks its {0} header")]
    MissingHeader(&'static str),

    #[error("a backplane message has unknown payload kind {0:?}")]
    UnknownKind(String),

    #[error("a text frame on the backplane is not UTF-8")]
    InvalidText(#[from] FromUtf8Error),

    #[error("no pending message {0} to acknowledge")]
    NoPendingMessage(String),

    #[error("the NATS server did not confirm this hub's subscriptions within {0:?}")]
    SubscriptionsUnconfirmed(std::time::Duration),
}

/// Convert any NATS client error into [`BackplaneError::Nats`], naming what was being attempted.
pub(crate) trait MapNatsErr<T> {
    fn map_nats_err(self, action: &'static str) -> Result<T, BackplaneError>;
}

/// Convert a failed NATS connection into [`BackplaneError::Connect`], naming the URL it was made to.
pub(crate) trait MapConnectErr<T> {
    fn map_connect_err(self, url: &str) -> Result<T, BackplaneError>;
}

impl<T> MapConnectErr<T> for Result<T, async_nats::ConnectError> {
    fn map_connect_err(self, url: &str) -> Result<T, BackplaneError> {
        self.map_err(|source| BackplaneError::Connect {
            url: redact_userinfo(url),
            source,
        })
    }
}

impl<T, E: std::error::Error + Send + Sync + 'static> MapNatsErr<T> for Result<T, E> {
    fn map_nats_err(self, action: &'static str) -> Result<T, BackplaneError> {
        self.map_err(|source| BackplaneError::Nats {
            action,
            source: Box::new(source),
        })
    }
}
