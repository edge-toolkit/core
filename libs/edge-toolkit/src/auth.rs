use base64::{Engine, engine::general_purpose::STANDARD as b64standard};
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
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
    pub fn new(username: String, password: SecretString) -> Self {
        Self { username, password }
    }

    /// Add authorisation header to HashMap.
    pub fn add_basic_auth_header(&self, headers: &mut std::collections::HashMap<String, String>) {
        let mut buf = String::default();
        b64standard.encode_string(
            format!("{}:{}", self.username, self.password.expose_secret()).as_bytes(),
            &mut buf,
        );
        headers.insert("authorization".to_string(), format!("Basic {buf}"));
    }
}
