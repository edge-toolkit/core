//! The crate's error type and its foreign-error conversions.

/// Errors raised by `et-int-gen`.
///
/// Every external error type that fallible functions can produce is wrapped
/// transparently via `#[from]`, so call sites just use `?`. Domain errors
/// (malformed schemas, missing `AsyncAPI` nodes, etc.) sit alongside as
/// non-transparent variants with static messages.
#[expect(
    clippy::exhaustive_enums,
    clippy::error_impl_error,
    reason = "internal crate (no SemVer); new variants land with their change, and crate::Error is the sole error type"
)]
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Command(#[from] command_error::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Yaml(#[from] serde_yaml::Error),
    #[error(transparent)]
    Http(#[from] reqwest::Error),
    #[error(transparent)]
    Semver(#[from] semver::Error),
    #[error(transparent)]
    Fmt(#[from] std::fmt::Error),
    #[error(transparent)]
    Regex(#[from] regex::Error),

    #[error("WS message JSON Schema malformed: {0}")]
    SchemaMalformed(&'static str),
    #[error("unsupported JSON Schema `type`: `{0}`")]
    UnsupportedSchemaType(String),
    #[error("enum value not a string in `{0}`")]
    EnumValueNotString(String),
    #[error("progenitor codegen: {0}")]
    Progenitor(#[from] progenitor::Error),
    #[error(transparent)]
    Syn(#[from] syn::Error),
    // `wit-parser` and the wasmtime bindgen return `anyhow::Result`; this variant is what lets `?` convert
    // one. The anyhow-only-in-error-rs semgrep rule keeps the name confined to this line.
    #[error(transparent)]
    Anyhow(#[from] anyhow::Error),
    #[error("zig codegen: {0}")]
    ZigCodegen(String),
}

// Left uncovered by design: `LanguageError` has a private field and no public constructor, and `set_language`
// only returns it on a tree-sitter/grammar ABI mismatch -- which would break every Zig-parsing test -- so this
// conversion cannot be exercised by a unit test.
impl From<tree_sitter::LanguageError> for Error {
    fn from(err: tree_sitter::LanguageError) -> Self {
        Self::ZigCodegen(format!("set Zig language: {err}"))
    }
}
