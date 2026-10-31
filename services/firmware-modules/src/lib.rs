//! The source of a firmware's `modules.rs`: every ws-module package it serves, built in with `include_bytes!`.
//!
//! A firmware's build script reads the variables the `std` server's modules service reads -- the module directories,
//! the one served at `/`, and the packages served from elsewhere -- and writes what [`generate`] returns. Each
//! directory is a module the way that service finds one: its `pkg/` when it has one, otherwise the directory itself,
//! named by the `package.json` there. Its files are the ones npm would publish, each served as its extension says and
//! tagged with a hash of its contents.

use std::fmt::Write as _;
use std::hash::{DefaultHasher, Hash as _, Hasher as _};
use std::path::{Component, Path, PathBuf};

use fs_err as fs;

/// The directory a module's build leaves its package in, when the module's own directory is its source.
const PACKAGE_DIR: &str = "pkg";
/// The manifest naming a module, and listing its files.
const PACKAGE_JSON: &str = "package.json";
/// Where the types the source builds its modules from live, named in full so a build with none imports nothing.
const MODULES_API: &str = "et_ws_server::picoserve_modules";
/// The variable the module directories come from, which errors about them name.
const MODULES_PATHS: &str = "MODULES_PATHS";

/// Why a firmware's modules cannot be built in.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ModulesError {
    #[error("{variable} must hold absolute paths, not {path}")]
    RelativePath { variable: String, path: PathBuf },
    #[error("{variable} holds a path that is not valid UTF-8")]
    NotUtf8 { variable: String },
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("{path} is not valid JSON")]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("{0} declares no name")]
    NoName(PathBuf),
    #[error("{0} lists a file that is not a string")]
    FileNotString(PathBuf),
    #[error("{manifest} lists {entry}, which is outside the package")]
    OutsidePackage { manifest: PathBuf, entry: String },
    #[error("{0} is a link to a file outside its package")]
    LinksOutside(PathBuf),
    #[error("MODULES_ROOT names {root}, which none of MODULES_PATHS provides; serving {names:?}")]
    UnknownRoot { root: String, names: Vec<String> },
    #[error("MODULES_ELSEWHERE needs MODULES_ELSEWHERE_URL to say where they are")]
    ElsewhereWithoutUrl,
}

/// The source of `modules.rs`, and every file it builds in, which the build has to rerun when one changes.
#[derive(Debug)]
#[non_exhaustive]
pub struct Generated {
    /// The Rust source defining `static MODULES`.
    pub source: String,
    /// Every file `source` names in an `include_bytes!`.
    pub files: Vec<PathBuf>,
}

/// The source building in the modules in `paths`, and serving the rest of the selection.
///
/// It serves `root` at `/`, and redirects the modules in `elsewhere` to `elsewhere_url`. `paths` and `elsewhere` are
/// comma-separated lists, as the `MODULES_PATHS` and `MODULES_ELSEWHERE` variables hold them, and every one of them may
/// be empty.
///
/// # Errors
///
/// When a path is relative or not UTF-8, a package cannot be read or lists or links to a file outside itself, `root`
/// names none of the modules, or `elsewhere` names modules with no `elsewhere_url` to find them at.
#[expect(
    clippy::use_debug,
    reason = "a str's Debug form is the Rust literal the generated source needs"
)]
pub fn generate(paths: &str, root: &str, elsewhere: &str, elsewhere_url: &str) -> Result<Generated, ModulesError> {
    let mut embedded = String::default();
    let mut names = Vec::new();
    let mut built_in = Vec::new();
    for path in items(paths) {
        let path = PathBuf::from(path);
        // Checked before anything is read, so a relative entry is refused as one, wherever the build happens to run.
        let _literal = include_path(MODULES_PATHS, &path)?;
        let package_dir = if path.join(PACKAGE_DIR).is_dir() {
            path.join(PACKAGE_DIR)
        } else {
            path
        };
        let package = package(&package_dir)?;
        let _written = writeln!(embedded, "    {MODULES_API}::Module::new({:?}, &[", package.name);
        for file in package.files {
            // Checked whole first, so the name within the package, a part of it, is valid UTF-8 too.
            let literal = include_path(MODULES_PATHS, &file)?;
            // Every file was collected from under the package, so it always has the package as its prefix.
            let relative = file
                .strip_prefix(&package_dir)
                .unwrap_or(&file)
                .to_string_lossy()
                .replace('\\', "/");
            let _written = writeln!(
                embedded,
                "        ({relative:?}, {MODULES_API}::ModuleFile::new({:?}, include_bytes!({literal}), {:?})),",
                content_type(&file),
                etag(&file)?,
            );
            built_in.push(file);
        }
        embedded.push_str("    ]),\n");
        names.push(package.name);
    }
    if !root.is_empty() && !names.iter().any(|name| name == root) {
        return Err(ModulesError::UnknownRoot {
            root: root.to_owned(),
            names,
        });
    }
    let elsewhere: Vec<&str> = items(elsewhere).collect();
    if !elsewhere.is_empty() && elsewhere_url.is_empty() {
        return Err(ModulesError::ElsewhereWithoutUrl);
    }
    let source = format!(
        "/// The modules the firmware serves, as `MODULES_PATHS`, `MODULES_ROOT` and `MODULES_ELSEWHERE*` named them.
static MODULES: {MODULES_API}::Modules = {MODULES_API}::Modules::new(
    &[
{embedded}    ],
    {root:?},
    &{elsewhere:?},
    {elsewhere_url:?},
);
"
    );
    Ok(Generated {
        source,
        files: built_in,
    })
}

/// `path` as the literal an `include_bytes!` names it by, which has to be absolute; `variable` is where it came from.
///
/// A build script runs in its own crate's directory, so a relative path would quietly resolve against that.
///
/// # Errors
///
/// When `path` is relative, or is not valid UTF-8.
pub fn include_path(variable: &str, path: &Path) -> Result<String, ModulesError> {
    if !path.is_absolute() {
        return Err(ModulesError::RelativePath {
            variable: variable.to_owned(),
            path: path.to_path_buf(),
        });
    }
    let path = path.to_str().ok_or_else(|| ModulesError::NotUtf8 {
        variable: variable.to_owned(),
    })?;
    Ok(format!("{path:?}"))
}

/// The comma-separated items of `list`, trimmed, with empty ones dropped.
fn items(list: &str) -> impl Iterator<Item = &str> {
    list.split(',').map(str::trim).filter(|item| !item.is_empty())
}

/// A module package: the name its `package.json` declares, and the files it publishes.
struct Package {
    name: String,
    files: Vec<PathBuf>,
}

/// The package in `dir`, with the files npm would publish from it.
///
/// That is every file its `package.json` lists in `files`, and the manifest itself, or every file in `dir` when it
/// lists none. Either way, nothing under `node_modules` or hidden.
#[expect(clippy::single_call_fn, reason = "the per-module step of generate")]
fn package(dir: &Path) -> Result<Package, ModulesError> {
    let manifest_path = dir.join(PACKAGE_JSON);
    // Matched rather than converted, so the error can name the manifest that would not parse.
    let manifest: serde_json::Value = match serde_json::from_str(&fs::read_to_string(&manifest_path)?) {
        Ok(manifest) => manifest,
        Err(source) => {
            return Err(ModulesError::Json {
                path: manifest_path,
                source,
            });
        }
    };
    let name = manifest
        .get("name")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| ModulesError::NoName(manifest_path.clone()))?
        .to_owned();
    let mut files = Vec::new();
    match manifest.get("files").and_then(serde_json::Value::as_array) {
        Some(listed) => {
            files.push(manifest_path.clone());
            for entry in listed {
                let entry = entry
                    .as_str()
                    .ok_or_else(|| ModulesError::FileNotString(manifest_path.clone()))?;
                // Only names within the package: a root, a drive or a `..` would build in a file from elsewhere.
                if !Path::new(entry)
                    .components()
                    .all(|part| matches!(part, Component::Normal(_) | Component::CurDir))
                {
                    return Err(ModulesError::OutsidePackage {
                        manifest: manifest_path,
                        entry: entry.to_owned(),
                    });
                }
                // Joined without its `.` parts, which would otherwise end up in the path the file is served at.
                let within: PathBuf = Path::new(entry)
                    .components()
                    .filter(|part| matches!(part, Component::Normal(_)))
                    .collect();
                collect(&dir.join(within), &mut files)?;
            }
        }
        None => collect(dir, &mut files)?,
    }
    // A link anywhere below the package root could build in a file from anywhere; one to the root itself is kept.
    let root = fs::canonicalize(dir)?;
    for file in &files {
        if !fs::canonicalize(file)?.starts_with(&root) {
            return Err(ModulesError::LinksOutside(file.clone()));
        }
    }
    files.sort();
    files.dedup();
    Ok(Package { name, files })
}

/// Add `path` to `files`, or every file under it when it is a directory, skipping `node_modules` and hidden entries.
fn collect(path: &Path, files: &mut Vec<PathBuf>) -> Result<(), ModulesError> {
    if !path.is_dir() {
        files.push(path.to_path_buf());
        return Ok(());
    }
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') || name == "node_modules" {
            continue;
        }
        collect(&entry.path(), files)?;
    }
    Ok(())
}

/// A tag for `file`'s contents, which changes whenever they do.
///
/// Only one firmware ever compares it with itself, so a hash that a later toolchain computes differently costs a
/// browser one refetch after an update and nothing more.
#[expect(
    clippy::single_call_fn,
    reason = "the tagging step of generate, one of each file's three fields"
)]
fn etag(file: &Path) -> Result<String, ModulesError> {
    let mut hasher = DefaultHasher::new();
    fs::read(file)?.hash(&mut hasher);
    Ok(format!("{:016x}", hasher.finish()))
}

/// The content type `file` is served as, by its extension, as a static file server would guess it.
#[expect(
    clippy::single_call_fn,
    reason = "the typing step of generate, one of each file's three fields"
)]
fn content_type(file: &Path) -> &'static str {
    match file.extension().and_then(|extension| extension.to_str()) {
        Some("html" | "htm") => "text/html; charset=utf-8",
        Some("js" | "mjs") => "application/javascript; charset=utf-8",
        Some("css") => "text/css",
        Some("json" | "map") => "application/json",
        Some("wasm") => "application/wasm",
        Some("png") => "image/png",
        Some("svg") => "image/svg+xml",
        Some("ico") => "image/x-icon",
        Some("txt" | "md" | "py" | "ts") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}
