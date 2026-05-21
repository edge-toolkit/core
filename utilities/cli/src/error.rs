use std::path::{Path, PathBuf};

use thiserror::Error;

/// Errors returned by `et-cli` operations. Variants carry the path or
/// value they failed on so users can see *what* went wrong, not just the
/// underlying `io::Error` text.
#[derive(Debug, Error)]
pub enum CliError {
    #[error("Failed to {op} {path:?}", op = op.describe())]
    Io {
        op: IoOp,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("Failed to resolve current working directory for {context}")]
    CurrentDir {
        context: &'static str,
        #[source]
        source: std::io::Error,
    },

    #[error("Failed to parse cluster input YAML")]
    ParseClusterYaml(#[from] serde_yaml::Error),

    #[error("Failed to parse {path}")]
    ParseToml {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },

    #[error("Failed to parse {path}")]
    ParseJson {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },

    #[error("Failed to serialize package JSON")]
    SerializeJson(#[source] serde_json::Error),

    #[error("Failed to serialize mise TOML")]
    SerializeToml(#[from] toml::ser::Error),

    #[error("Expected pyproject.toml or Cargo.toml in module directory {0:?}")]
    MissingManifest(PathBuf),

    #[error("Output path {0:?} has no parent directory")]
    NoParentDir(PathBuf),

    #[error("{0} has no [package] section")]
    MissingPackageSection(PathBuf),

    #[error("{0} contains a non-object dependencies field")]
    NonObjectDependencies(PathBuf),

    #[error("main = {main:?} does not exist in {dir}")]
    MissingMainFile { main: String, dir: PathBuf },

    #[error(
        "No main file in {dir}; expected {underscored}.{ext} or {hyphenated}.{ext} (override with [ws-module] main)"
    )]
    UnresolvedMainFile {
        dir: PathBuf,
        underscored: String,
        hyphenated: String,
        ext: &'static str,
    },

    #[error("{0} must contain a JSON object")]
    NonObjectPackageJson(PathBuf),

    #[error("Verification root {root:?} maps multiple scenario inputs to the same output directory {output:?}")]
    DuplicateScenarioOutput { root: PathBuf, output: PathBuf },

    #[error("Verification input file {0:?} has no file stem")]
    MissingFileStem(PathBuf),

    #[error("Verification root {0:?} does not contain any scenario files under */input/*.yaml or */input/*.yml")]
    NoScenarios(PathBuf),

    #[error("Unsupported deployment_type {0:?}. Supported values are currently: mise, docker-compose")]
    UnsupportedDeploymentType(String),

    #[error("No local module or runtime package found for dependency {0:?}")]
    UnknownDependency(String),
}

/// Categories of filesystem operation, used so a single I/O failure variant
/// can render a useful message ("Failed to read X", "Failed to create
/// directory Y", …) without one variant per call site.
#[derive(Debug, Clone, Copy)]
pub enum IoOp {
    Read,
    Write,
    CreateDir,
    ReadDir,
    ReadDirEntry,
    FileType,
}

impl IoOp {
    pub fn describe(&self) -> &'static str {
        match self {
            IoOp::Read => "read",
            IoOp::Write => "write",
            IoOp::CreateDir => "create directory",
            IoOp::ReadDir => "read directory",
            IoOp::ReadDirEntry => "read entry from",
            IoOp::FileType => "read file type for",
        }
    }
}

/// Extension over [`std::io::Result`] that turns a bare [`std::io::Error`]
/// into a [`CliError::Io`] carrying the failing path and operation kind.
/// This is the only spot in the crate that converts io errors — everywhere
/// else uses `.io_context(IoOp::..., path)?`, no `map_err` needed.
pub trait IoResultExt<T> {
    fn io_context(self, op: IoOp, path: impl AsRef<Path>) -> Result<T, CliError>;
}

impl<T> IoResultExt<T> for std::io::Result<T> {
    fn io_context(self, op: IoOp, path: impl AsRef<Path>) -> Result<T, CliError> {
        match self {
            Ok(value) => Ok(value),
            Err(source) => Err(CliError::Io {
                op,
                path: path.as_ref().to_path_buf(),
                source,
            }),
        }
    }
}

/// Wrap [`std::env::current_dir`] in our error type. Replaces the
/// `map_err(|source| CliError::CurrentDir { context, source })` boilerplate.
pub fn current_dir_for(context: &'static str) -> Result<PathBuf, CliError> {
    match std::env::current_dir() {
        Ok(dir) => Ok(dir),
        Err(source) => Err(CliError::CurrentDir { context, source }),
    }
}

/// Parse `src` as TOML into `T`, attaching `path` to the error on failure.
/// Replaces a `.map_err(...)` at every call site.
pub fn parse_toml<T>(path: impl AsRef<Path>, src: &str) -> Result<T, CliError>
where
    T: for<'de> serde::Deserialize<'de>,
{
    match toml::from_str(src) {
        Ok(value) => Ok(value),
        Err(source) => Err(CliError::ParseToml {
            path: path.as_ref().to_path_buf(),
            source,
        }),
    }
}

/// Parse `src` as JSON into `T`, attaching `path` to the error on failure.
pub fn parse_json<T>(path: impl AsRef<Path>, src: &str) -> Result<T, CliError>
where
    T: for<'de> serde::Deserialize<'de>,
{
    match serde_json::from_str(src) {
        Ok(value) => Ok(value),
        Err(source) => Err(CliError::ParseJson {
            path: path.as_ref().to_path_buf(),
            source,
        }),
    }
}

/// Serialize `value` as pretty JSON, surfacing the failure as
/// [`CliError::SerializeJson`]. There's no input path to attach.
pub fn serialize_json_pretty<T: serde::Serialize>(value: &T) -> Result<String, CliError> {
    match serde_json::to_string_pretty(value) {
        Ok(out) => Ok(out),
        Err(source) => Err(CliError::SerializeJson(source)),
    }
}
