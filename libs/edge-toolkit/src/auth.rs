use base64::{Engine as _, engine::general_purpose::STANDARD as b64standard};
use secrecy::{ExposeSecret as _, SecretString};
use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
#[non_exhaustive]
/// Basic Authentication config.
pub struct BasicAuth {
    /// Username.
    pub username: String,
    /// Password.
    pub password: SecretString,
}

impl BasicAuth {
    /// Create a new `BasicAuth` instance.
    #[must_use]
    pub const fn new(username: String, password: SecretString) -> Self {
        Self { username, password }
    }

    /// Add authorisation header to `HashMap`.
    pub fn add_basic_auth_header(&self, headers: &mut std::collections::HashMap<String, String>) {
        let mut buf = String::default();
        b64standard.encode_string(
            format!("{}:{}", self.username, self.password.expose_secret()).as_bytes(),
            &mut buf,
        );
        let _previous = headers.insert("authorization".to_string(), format!("Basic {buf}"));
    }
}
