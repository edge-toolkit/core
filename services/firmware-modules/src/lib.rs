//! What a firmware's build script generates: `modules.rs`, every ws-module package it serves, built in with
//! `include_bytes!`, and what the board it is built for bakes in and links with.
//!
//! A firmware's build script reads the variables the `std` server's modules service reads -- the module directories,
//! the one served at `/`, and the packages served from elsewhere -- and writes what [`generate`] returns. Each
//! directory is a module the way that service finds one: its `pkg/` when it has one, otherwise the directory itself,
//! named by the `package.json` there. Its files are the ones npm would publish, each served as its extension says and
//! tagged with a hash of its contents. Built for a board, it also hands [`board`] the board's variables.

use std::env::VarError;
use std::fmt::Write as _;
use std::hash::{DefaultHasher, Hash as _, Hasher as _};
use std::path::{Component, Path, PathBuf};

use edge_toolkit::board::Guest;
use et_ws_wasi_runner::engine::precompile;
use fs_err as fs;

/// The variables [`board`] reads, which a build script has to rerun when one changes.
pub const BOARD_VARIABLES: [&str; 3] = [CYW43_FIRMWARE_DIR, WIFI_NETWORK, WIFI_PASSWORD];
/// The directory holding the Pico 2 W radio's blobs.
const CYW43_FIRMWARE_DIR: &str = "CYW43_FIRMWARE_DIR";
/// The network a board joins.
const WIFI_NETWORK: &str = "WIFI_NETWORK";
/// The network's passphrase.
const WIFI_PASSWORD: &str = "WIFI_PASSWORD";

/// The directory a module's build leaves its package in, when the module's own directory is its source.
const PACKAGE_DIR: &str = "pkg";
/// The manifest naming a module, and listing its files.
const PACKAGE_JSON: &str = "package.json";
/// Where the types the source builds its modules from live, named in full so a build with none imports nothing.
const MODULES_API: &str = "et_ws_server::picoserve_modules";
/// The variable the module directories come from, which errors about them name.
const MODULES_PATHS: &str = "MODULES_PATHS";
/// The variable naming the runners a board starts and the modules they run, which errors about them name.
pub const RUNNER_GUESTS: &str = "RUNNER_GUESTS";
/// The Pulley target of a 32-bit board, whose pointers, and so table elements, are four bytes rather than eight.
const PULLEY32: &str = "pulley32";

/// Why a firmware's modules cannot be built in, or its board's build cannot be generated.
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
    #[error("a board's firmware needs {0} set when it is built")]
    Unset(String),
    #[error("{variable} holds no {file}")]
    MissingBlob { variable: String, file: String },
    #[error("no board is built for {0}")]
    NoBoard(String),
    #[error("RUNNER_GUESTS holds {0:?}, which is not a runner=package pair")]
    NotAGuest(String),
    #[error("runner {runner} runs {package}, which none of MODULES_PATHS provides; serving {names:?}")]
    UnknownGuest {
        runner: String,
        package: String,
        names: Vec<String>,
    },
    #[error("runner {runner} runs {package}, whose main {main:?} is not a WebAssembly component")]
    NotAComponent {
        runner: String,
        package: String,
        main: String,
    },
    #[error("{path} is not a WebAssembly component")]
    Unreadable {
        path: PathBuf,
        #[source]
        source: wasmparser::BinaryReaderError,
    },
    #[error("runner {runner} imports {imports:?}, which this board does not host")]
    Unhosted { runner: String, imports: Vec<String> },
    #[error("the runners' guests start with {needed} bytes of memories and tables, but this board holds {budget}")]
    GuestMemory { needed: u64, budget: u64 },
    #[error("runner {runner}'s guest could not be compiled for the board: {reason}")]
    Precompile { runner: String, reason: String },
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
        let package_dir = package_dir(path);
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

/// The source building in a guest for each runner in `guests`, compiled for `target`, and checked against the board.
///
/// `guests` is a comma-separated list of `runner=package` pairs, as `RUNNER_GUESTS` holds it, each naming a module in
/// `paths` whose `main` is the component to run. A board hosts only what [`edge_toolkit::board::HOSTED_IMPORTS`] lists,
/// and holds `memory` bytes for its guests' memories and tables, so a guest importing anything else, or guests starting
/// with more between them, are refused here rather than when the board instantiates them. A table element is counted
/// at the pointer width of `target`, as the board's runner charges it. Each compiled guest is written to `out_dir`, and
/// the source defines `const GUESTS`, every runner's name and its compiled component, as an array so the firmware can
/// size a task pool by it.
///
/// # Errors
///
/// When a pair is malformed, a package is not in `paths` or its `main` is not a readable component, a guest imports
/// what the board does not host, the guests need more than `memory`, or one does not compile for `target`.
#[expect(
    clippy::use_debug,
    reason = "a str's Debug form is the Rust literal the generated source needs"
)]
pub fn guests(paths: &str, guests: &str, target: &str, memory: u64, out_dir: &Path) -> Result<Generated, ModulesError> {
    // Checked before anything is written, so every compiled guest's path is absolute UTF-8 too.
    let _literal = include_path(RUNNER_GUESTS, out_dir)?;
    let packages = items(paths)
        .map(|path| runnable(&package_dir(PathBuf::from(path))))
        .collect::<Result<Vec<_>, _>>()?;
    let pointer_bytes = if target == PULLEY32 { 4 } else { 8 };
    let mut entries = String::default();
    let mut components = Vec::new();
    let mut needed: u64 = 0;
    for (index, pair) in items(guests).enumerate() {
        let (runner, component, bytes, guest) = checked_guest(pair, &packages)?;
        needed = needed.saturating_add(guest.starts_with(pointer_bytes));
        // Matched rather than converted, so the error can name the runner whose guest would not compile.
        let compiled = match precompile(target, &bytes) {
            Ok(compiled) => compiled,
            Err(error) => {
                return Err(ModulesError::Precompile {
                    runner: runner.to_owned(),
                    reason: error.to_string(),
                });
            }
        };
        let compiled_path = out_dir.join(format!("guest-{index}.cwasm"));
        fs::write(&compiled_path, compiled)?;
        let literal = compiled_path.display().to_string();
        let _written = writeln!(entries, "    ({runner:?}, include_bytes!({literal:?})),");
        components.push(component);
    }
    if needed > memory {
        return Err(ModulesError::GuestMemory { needed, budget: memory });
    }
    let count = components.len();
    let source = format!(
        "/// The guests the firmware runs beside the hub, each compiled for it, as `RUNNER_GUESTS` named them.
pub(crate) const GUESTS: [(&str, &[u8]); {count}] = [
{entries}];
/// The bytes the board holds for its guests' memories, which they draw on as they grow, whatever they start with.
pub(crate) const GUEST_MEMORY: u64 = {budget};
",
        budget = separated(memory)
    );
    Ok(Generated {
        source,
        files: components,
    })
}

/// `value` as an integer literal with its digits in groups of three, as the firmware's lints ask of a long one.
#[expect(
    clippy::single_call_fn,
    reason = "the literal step of guests, kept apart from what it writes"
)]
fn separated(value: u64) -> String {
    let digits = value.to_string();
    let mut groups: Vec<&[u8]> = digits.as_bytes().rchunks(3).collect();
    groups.reverse();
    groups
        .iter()
        .map(|group| String::from_utf8_lossy(group))
        .collect::<Vec<_>>()
        .join("_")
}

/// Refuse `entry`, a path the manifest at `manifest` names, unless it names something within the package.
///
/// A root, a drive or a `..` would build in a file from elsewhere.
fn within_package(entry: &str, manifest: &Path) -> Result<(), ModulesError> {
    if Path::new(entry)
        .components()
        .all(|part| matches!(part, Component::Normal(_) | Component::CurDir))
    {
        return Ok(());
    }
    Err(ModulesError::OutsidePackage {
        manifest: manifest.to_path_buf(),
        entry: entry.to_owned(),
    })
}

/// A package a runner may run: its name, the path of its `main`, and `main` as the manifest spells it.
struct Runnable {
    name: String,
    component: PathBuf,
    main: String,
}

/// The package in `dir`, as a runner would find its component there.
#[expect(
    clippy::single_call_fn,
    reason = "the per-module step of guests, kept apart from its runners' checks"
)]
fn runnable(dir: &Path) -> Result<Runnable, ModulesError> {
    let manifest_path = dir.join(PACKAGE_JSON);
    let manifest = manifest(&manifest_path)?;
    let name = manifest_name(&manifest, &manifest_path)?;
    // No `main` is no component, which is refused for the runner that names the package rather than here.
    let main = manifest
        .get("main")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    within_package(main, &manifest_path)?;
    Ok(Runnable {
        name,
        component: dir.join(main),
        main: main.to_owned(),
    })
}

/// The runner `pair` names, once a board can host its component.
///
/// With it, the component's path and bytes, and what the component asks of the board.
#[expect(
    clippy::single_call_fn,
    reason = "the per-runner check of guests, kept apart from compiling"
)]
fn checked_guest<'pair>(
    pair: &'pair str,
    packages: &[Runnable],
) -> Result<(&'pair str, PathBuf, Vec<u8>, Guest), ModulesError> {
    let (runner, package) = pair
        .split_once('=')
        .map(|(runner, package)| (runner.trim(), package.trim()))
        .filter(|(runner, package)| !runner.is_empty() && !package.is_empty())
        .ok_or_else(|| ModulesError::NotAGuest(pair.to_owned()))?;
    let Some(found) = packages.iter().find(|candidate| candidate.name == package) else {
        return Err(ModulesError::UnknownGuest {
            runner: runner.to_owned(),
            package: package.to_owned(),
            names: packages.iter().map(|candidate| candidate.name.clone()).collect(),
        });
    };
    if found.component.extension().and_then(|extension| extension.to_str()) != Some("wasm") {
        return Err(ModulesError::NotAComponent {
            runner: runner.to_owned(),
            package: package.to_owned(),
            main: found.main.clone(),
        });
    }
    let bytes = fs::read(&found.component)?;
    let guest = match Guest::read(&bytes) {
        Ok(guest) => guest,
        Err(source) => {
            return Err(ModulesError::Unreadable {
                path: found.component.clone(),
                source,
            });
        }
    };
    let unhosted: Vec<String> = guest.unhosted().map(str::to_owned).collect();
    if !unhosted.is_empty() {
        return Err(ModulesError::Unhosted {
            runner: runner.to_owned(),
            imports: unhosted,
        });
    }
    Ok((runner, found.component.clone(), bytes, guest))
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

/// Write what a firmware for the board `arch` picks bakes in to `out_dir`, and return the directives that link it.
///
/// Every board gets `wifi.rs`, the network `WIFI_NETWORK` and `WIFI_PASSWORD` name, each read through `var` as from
/// the environment. Bare-metal Arm is the Raspberry Pi Pico 2 W, which also gets its `memory_x` layout and `cyw43.rs`,
/// its radio's blobs from `CYW43_FIRMWARE_DIR`, and links with cortex-m-rt's script; bare-metal RISC-V is the ESP32-C6,
/// which links with esp-hal's. Both link defmt's too.
///
/// # Errors
///
/// When `arch` is no board's, a variable the board needs is unset, `CYW43_FIRMWARE_DIR` lacks a blob or is not an
/// absolute UTF-8 path, or a file cannot be written.
pub fn board<V>(arch: &str, out_dir: &Path, memory_x: &[u8], var: V) -> Result<Vec<String>, ModulesError>
where
    V: Fn(&str) -> Result<String, VarError>,
{
    let required = |name: &str| {
        let Ok(value) = var(name) else {
            return Err(ModulesError::Unset(name.to_owned()));
        };
        Ok(value)
    };
    let mut directives = Vec::new();
    let script = match arch {
        "arm" => {
            fs::write(out_dir.join("memory.x"), memory_x)?;
            let blobs = cyw43(Path::new(&required(CYW43_FIRMWARE_DIR)?))?;
            fs::write(out_dir.join("cyw43.rs"), blobs)?;
            directives.push(format!("cargo:rustc-link-search={}", out_dir.display()));
            directives.push("cargo:rustc-link-arg-bins=--nmagic".to_owned());
            "link.x"
        }
        "riscv32" => "linkall.x",
        _ => return Err(ModulesError::NoBoard(arch.to_owned())),
    };
    let network = required(WIFI_NETWORK)?;
    let password = required(WIFI_PASSWORD)?;
    let wifi = format!(
        "/// The network the board joins.
const WIFI_NETWORK: &str = {network:?};
/// The network's passphrase.
const WIFI_PASSWORD: &str = {password:?};
"
    );
    fs::write(out_dir.join("wifi.rs"), wifi)?;
    directives.push(format!("cargo:rustc-link-arg-bins=-T{script}"));
    directives.push("cargo:rustc-link-arg-bins=-Tdefmt.x".to_owned());
    Ok(directives)
}

/// The source of `cyw43.rs`: the Pico 2 W radio's blobs, from `dir`.
#[expect(
    clippy::single_call_fn,
    reason = "the radio step of board, the one board with blobs of its own"
)]
fn cyw43(dir: &Path) -> Result<String, ModulesError> {
    let blob = |file: &str| {
        let path = dir.join(file);
        if !path.is_file() {
            return Err(ModulesError::MissingBlob {
                variable: CYW43_FIRMWARE_DIR.to_owned(),
                file: file.to_owned(),
            });
        }
        Ok(format!("include_bytes!({})", include_path(CYW43_FIRMWARE_DIR, &path)?))
    };
    let firmware = blob("43439A0.bin")?;
    let clm = blob("43439A0_clm.bin")?;
    let nvram = blob("nvram_rp2040.bin")?;
    Ok(format!(
        "/// The CYW43439's firmware.
static RADIO_FIRMWARE: &cyw43::Aligned<cyw43::A4, [u8]> = &cyw43::Aligned(*{firmware});
/// The CYW43439's country locale matrix.
static RADIO_CLM: &[u8] = {clm};
/// The CYW43439's configuration for the Pico 2 W's module.
static RADIO_NVRAM: &cyw43::Aligned<cyw43::A4, [u8]> = &cyw43::Aligned(*{nvram});
"
    ))
}

/// The comma-separated items of `list`, trimmed, with empty ones dropped.
fn items(list: &str) -> impl Iterator<Item = &str> {
    list.split(',').map(str::trim).filter(|item| !item.is_empty())
}

/// The directory holding the package of the module at `path`: its `pkg/` when it has one, otherwise `path` itself.
fn package_dir(path: PathBuf) -> PathBuf {
    if path.join(PACKAGE_DIR).is_dir() {
        path.join(PACKAGE_DIR)
    } else {
        path
    }
}

/// The `package.json` at `path`, parsed.
fn manifest(path: &Path) -> Result<serde_json::Value, ModulesError> {
    // Matched rather than converted, so the error can name the manifest that would not parse.
    match serde_json::from_str(&fs::read_to_string(path)?) {
        Ok(manifest) => Ok(manifest),
        Err(source) => Err(ModulesError::Json {
            path: path.to_path_buf(),
            source,
        }),
    }
}

/// The name `manifest`, read from `path`, declares.
fn manifest_name(manifest: &serde_json::Value, path: &Path) -> Result<String, ModulesError> {
    Ok(manifest
        .get("name")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| ModulesError::NoName(path.to_path_buf()))?
        .to_owned())
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
    let manifest = manifest(&manifest_path)?;
    let name = manifest_name(&manifest, &manifest_path)?;
    let mut files = Vec::new();
    match manifest.get("files").and_then(serde_json::Value::as_array) {
        Some(listed) => {
            files.push(manifest_path.clone());
            for entry in listed {
                let entry = entry
                    .as_str()
                    .ok_or_else(|| ModulesError::FileNotString(manifest_path.clone()))?;
                within_package(entry, &manifest_path)?;
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
