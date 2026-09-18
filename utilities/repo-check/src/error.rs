//! The crate's error type and its foreign-error conversions.

/// Errors raised by `et-repo-check`.
#[derive(Debug, thiserror::Error)]
#[expect(
    clippy::exhaustive_enums,
    clippy::error_impl_error,
    reason = "one internal binary with no SemVer surface; every check returns this type and adds variants with itself"
)]
pub enum Error {
    /// Carries the count so the process exits non-zero once, after every check has had its say.
    #[error("repo-check: {0} problem(s) found")]
    Failures(usize),

    #[error(transparent)]
    Command(#[from] command_error::Error),
    #[error(transparent)]
    Regex(#[from] regex::Error),
    #[error(transparent)]
    Utf8(#[from] std::string::FromUtf8Error),
}
