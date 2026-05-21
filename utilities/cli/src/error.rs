use std::path::{Path, PathBuf};

use thiserror::Error;

/// Errors returned by `et-cli` operations.
///
/// Variants carry the path or value they failed on so users can see *what*
/// went wrong, not just the underlying error text. `Io` is
/// `#[from]`-forwarded — the inner `std::io::Error` arrives from `fs_err`,
/// which already embeds the failing path in its `Display`, so we don't need
/// a path field here.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum CliError {
    #[error(transparent)]
    Io(#[from] std::io::Error),

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

/// Parse `src` as TOML into `T`, attaching `path` to the error on failure.
/// Replaces a `.map_err(...)` at every call site.
pub fn parse_toml<T, P: AsRef<Path>>(path: P, src: &str) -> Result<T, CliError>
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
pub fn parse_json<T, P: AsRef<Path>>(path: P, src: &str) -> Result<T, CliError>
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
