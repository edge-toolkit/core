use std::path::PathBuf;

use thiserror::Error;

/// Errors returned by `et-cli` operations. Variants carry the path or
/// value they failed on so users can see *what* went wrong, not just the
/// underlying `io::Error` text.
#[derive(Debug, Error)]
pub enum CliError {
    #[error("Failed to read input file: {path:?}")]
    ReadInput {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("Failed to read {path}")]
    ReadFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("Failed to write {path}")]
    WriteFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("Failed to write output file: {path:?}")]
    WriteOutput {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("Failed to create output directory: {path:?}")]
    CreateOutputDir {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("Failed to read verification root directory: {path:?}")]
    ReadVerificationRoot {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("Failed to read verification input directory: {path:?}")]
    ReadVerificationInputDir {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("Failed to read entry from {path:?}")]
    ReadDirEntry {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("Failed to read file type for {path:?}")]
    ReadFileType {
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
    ParseClusterYaml(#[source] serde_yaml::Error),

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
    SerializeToml(#[source] toml::ser::Error),

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
