//! Provenance for every WIT package under `generated/specs/wit/deps/` that
//! isn't generated from `ClientMessage` / `ServerMessage`.
//!
//! Six WASI packages (`wasi-clocks`, `wasi-io`, `wasi-keyvalue`,
//! `wasi-logging`, `wasi-nn`, `wasi-webgpu`) are fetched verbatim from
//! upstream `WebAssembly/<repo>` at pinned tags or commit SHAs.
//!
//! `mise run fetch-wit-deps` re-runs everything; `gen-specs-check` flags
//! drift.

use std::path::Path;

use fs_err as fs;

use crate::Error;

/// One file within an upstream package.
struct File {
    name: &'static str,
}

/// An upstream WASI WIT package pinned to a tag or commit SHA.
struct UpstreamPackage {
    /// Directory name under `generated/specs/wit/deps/`.
    local_dir: &'static str,
    /// `WebAssembly/<repo>` (always under the WebAssembly org).
    repo: &'static str,
    /// Tag (e.g. `v0.2.6`) or commit SHA.
    /// wasi-logging has no releases, so it pins by SHA; everything else by release tag.
    git_ref: &'static str,
    files: &'static [File],
}

const PACKAGES: &[UpstreamPackage] = &[
    UpstreamPackage {
        local_dir: "wasi-clocks",
        repo: "wasi-clocks",
        git_ref: "v0.2.8",
        files: &[
            File {
                name: "monotonic-clock.wit",
            },
            File { name: "timezone.wit" },
            File { name: "wall-clock.wit" },
            File { name: "world.wit" },
        ],
    },
    UpstreamPackage {
        local_dir: "wasi-io",
        repo: "wasi-io",
        git_ref: "v0.2.8",
        files: &[
            File { name: "error.wit" },
            File { name: "poll.wit" },
            File { name: "streams.wit" },
            File { name: "world.wit" },
        ],
    },
    UpstreamPackage {
        local_dir: "wasi-keyvalue",
        repo: "wasi-keyvalue",
        git_ref: "v0.2.0-draft",
        files: &[
            File { name: "atomic.wit" },
            File { name: "batch.wit" },
            File { name: "store.wit" },
            File { name: "watch.wit" },
            File { name: "world.wit" },
        ],
    },
    UpstreamPackage {
        local_dir: "wasi-logging",
        repo: "wasi-logging",
        // No release tags exist; pinned to a known-good commit.
        git_ref: "d31c41d0d9eed81aabe02333d0025d42acf3fb75",
        files: &[File { name: "logging.wit" }, File { name: "world.wit" }],
    },
    UpstreamPackage {
        local_dir: "wasi-nn",
        repo: "wasi-nn",
        git_ref: "0.2.0-rc-2024-10-28",
        files: &[File { name: "wasi-nn.wit" }],
    },
    // The `imports` world rides along so a consumer can `include` the whole
    // interface set instead of naming `wasi:webgpu/webgpu` directly; the
    // runner world imports the interface, `wasi-webgpu-wasmtime` binds the
    // world.
    UpstreamPackage {
        local_dir: "wasi-webgpu",
        repo: "wasi-gfx",
        git_ref: "v0.3.0-rc.2",
        files: &[File { name: "imports.wit" }, File { name: "webgpu.wit" }],
    },
];

/// Refresh every upstream WIT package and write them all under `generated/specs/wit/deps/<pkg>/`.
/// Triggered by `mise run fetch-wit-deps`.
pub fn run(project_root: &Path) -> Result<(), Error> {
    let deps_root = project_root.join("generated/specs/wit/deps");
    for pkg in PACKAGES {
        fetch_one(&deps_root, pkg)?;
    }
    Ok(())
}

#[expect(
    clippy::single_call_fn,
    clippy::print_stdout,
    reason = "helper called once by run(); et-int-gen is a CLI, stdout progress lines are intended user-facing output"
)]
fn fetch_one(deps_root: &Path, pkg: &UpstreamPackage) -> Result<(), Error> {
    let dest = deps_root.join(pkg.local_dir);
    // Wipe the destination first -- guards against orphan files left over
    // from an old pin that the new pin no longer ships.
    if dest.exists() {
        fs::remove_dir_all(&dest)?;
    }
    fs::create_dir_all(&dest)?;
    for file in pkg.files {
        let url = format!(
            "https://raw.githubusercontent.com/WebAssembly/{repo}/{git_ref}/wit/{file}",
            repo = pkg.repo,
            git_ref = pkg.git_ref,
            file = file.name,
        );
        let body = reqwest::blocking::get(&url)?.error_for_status()?.text()?;
        let target = dest.join(file.name);
        fs::write(&target, body)?;
        println!("wrote {}", target.display());
    }
    Ok(())
}
