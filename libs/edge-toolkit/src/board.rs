//! What a board's firmware runner hosts for a guest, and the reading of a guest component that checks one against it.
//!
//! A board links only the interfaces below, so a guest importing any other fails to instantiate there. Reading the
//! guest ahead of time lets a deployment refuse it before anything is built or flashed.

/// The WASI Preview 2 release a board's runner links its `wasi:` interfaces at.
pub const PREVIEW2: &str = "0.2.12";

/// Every interface a board's runner links, at the version it links it: all a guest may import to run on a board.
///
/// The `runner` world's own imports, and the WASI Preview 2 a ws-module guest built for `wasm32-wasip2` imports, the
/// latter at [`PREVIEW2`].
pub const HOSTED_IMPORTS: [&str; 19] = [
    "et:ws-messages/messages@0.2.0",
    "et:ws-wasi/ws@0.2.0",
    "wasi:cli/environment@0.2.12",
    "wasi:cli/exit@0.2.12",
    "wasi:cli/stderr@0.2.12",
    "wasi:cli/stdin@0.2.12",
    "wasi:cli/stdout@0.2.12",
    "wasi:cli/terminal-input@0.2.12",
    "wasi:cli/terminal-output@0.2.12",
    "wasi:cli/terminal-stderr@0.2.12",
    "wasi:cli/terminal-stdin@0.2.12",
    "wasi:cli/terminal-stdout@0.2.12",
    "wasi:clocks/monotonic-clock@0.2.12",
    "wasi:clocks/wall-clock@0.2.12",
    "wasi:io/error@0.2.12",
    "wasi:io/poll@0.2.12",
    "wasi:io/streams@0.2.12",
    "wasi:keyvalue/store@0.2.0-draft",
    "wasi:logging/logging@0.1.0-draft",
];

/// What a guest instrumented for coverage imports beyond [`HOSTED_IMPORTS`], to write its profile into `/cov`.
///
/// A board's runner links these only when built for coverage, which the repo's coverage workflow does to run the
/// instrumented guests; a board's firmware never is.
pub const COVERAGE_IMPORTS: [&str; 2] = ["wasi:filesystem/preopens@0.2.12", "wasi:filesystem/types@0.2.12"];

/// Every interface this build's board runner links: [`HOSTED_IMPORTS`].
#[cfg(not(feature = "coverage"))]
#[expect(
    clippy::single_call_fn,
    reason = "one of two builds of the list, which `hosts` reads either way"
)]
fn linked() -> impl Iterator<Item = &'static &'static str> {
    HOSTED_IMPORTS.iter()
}

/// Every interface this build's board runner links: [`HOSTED_IMPORTS`], and [`COVERAGE_IMPORTS`] with them.
#[cfg(feature = "coverage")]
#[expect(
    clippy::single_call_fn,
    reason = "one of two builds of the list, which `hosts` reads either way"
)]
fn linked() -> impl Iterator<Item = &'static &'static str> {
    HOSTED_IMPORTS.iter().chain(&COVERAGE_IMPORTS)
}

/// Whether a board hosts `import`, an interface as a component imports it, `name@version`.
///
/// It does when the board links the same interface at a version in the same semver series, which is what wasmtime's
/// linker resolves an import to: one that names no version, or another series, fails to instantiate.
#[must_use]
pub fn hosts(import: &str) -> bool {
    let Some((name, version)) = import.split_once('@') else {
        return false;
    };
    linked().any(|hosted| {
        hosted
            .split_once('@')
            .is_some_and(|(hosted_name, linked)| hosted_name == name && series(linked) == series(version))
    })
}

/// The part of `version` a compatible one shares, as Cargo's semver rules have it, and wasmtime's linker with them.
///
/// From `1.0.0` on that is the major version, and below it the minor, so `0.2.0` and `0.2.12` are one series but
/// `0.3.0` another; a `0.0` version, a prerelease or one with build metadata matches only itself.
fn series(version: &str) -> (&str, &str) {
    if version.contains(['-', '+']) {
        return (version, "");
    }
    let Some((major, rest)) = version.split_once('.') else {
        return (version, "");
    };
    if major != "0" {
        return (major, "");
    }
    let minor = rest.split_once('.').map_or(rest, |(minor, _patch)| minor);
    if minor == "0" {
        return (version, "");
    }
    (major, minor)
}

/// What a guest component asks of the runner that hosts it.
#[cfg(feature = "std")]
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct Guest {
    /// The interfaces the component imports, as it names them, in the order it declares them.
    pub imports: Vec<String>,
    /// The bytes its core modules' memories start at, which the runner allocates before the guest runs.
    pub memory: u64,
    /// The elements its core modules' tables start with, each a pointer the runner allocates beside the memories.
    pub table_elements: u64,
}

#[cfg(feature = "std")]
impl Guest {
    /// Read what `component`, a WebAssembly component's bytes, imports and how much memory it starts with.
    ///
    /// Only the outer component's imports count, since a nested one's are satisfied inside the component. A memory or
    /// table a core module imports is another module's, already counted where it is defined.
    ///
    /// # Errors
    ///
    /// When `component` is not a WebAssembly binary.
    pub fn read(component: &[u8]) -> wasmparser::Result<Self> {
        use wasmparser::{Parser, Payload};

        let mut imports = Vec::new();
        let mut memory: u64 = 0;
        let mut table_elements: u64 = 0;
        let mut depth: usize = 0;
        for payload in Parser::new(0).parse_all(component) {
            let payload = payload?;
            if matches!(
                payload,
                Payload::ModuleSection { .. } | Payload::ComponentSection { .. }
            ) {
                depth = depth.saturating_add(1);
            } else if matches!(payload, Payload::End(_)) {
                depth = depth.saturating_sub(1);
            } else if let Payload::ComponentImportSection(section) = payload {
                if depth == 0 {
                    for import in section {
                        imports.push(String::from(import?.name.name));
                    }
                }
            } else if let Payload::MemorySection(section) = payload {
                for declared in section {
                    let declared = declared?;
                    let page_size = 1_u64 << declared.page_size_log2.unwrap_or(16);
                    memory = memory.saturating_add(declared.initial.saturating_mul(page_size));
                }
            } else if let Payload::TableSection(section) = payload {
                for declared in section {
                    table_elements = table_elements.saturating_add(declared?.ty.initial);
                }
            } else {
                // Every other section declares neither an import of the component's, a memory nor a table.
            }
        }
        Ok(Self {
            imports,
            memory,
            table_elements,
        })
    }

    /// The bytes the runner allocates for this guest's memories and tables before it runs.
    ///
    /// A table element is a pointer, `pointer_bytes` wide on the target the guest runs on.
    #[must_use]
    pub const fn starts_with(&self, pointer_bytes: u64) -> u64 {
        self.memory
            .saturating_add(self.table_elements.saturating_mul(pointer_bytes))
    }

    /// The imports a board does not host, which would stop this guest instantiating there.
    pub fn unhosted(&self) -> impl Iterator<Item = &str> {
        self.imports.iter().map(String::as_str).filter(|import| !hosts(import))
    }
}

/// The lock a board keeps shared state under: a spin lock, since a board has no `std::sync::Mutex`.
///
/// Nothing on a board can poison it, so its `lock()` gives the guard itself. It is the same lock in a host build of a
/// board's code, so that code behaves there as it will on the board.
#[cfg(feature = "board")]
pub type BoardLock<T> = spin::Mutex<T>;
