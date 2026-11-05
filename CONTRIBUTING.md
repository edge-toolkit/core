# Contributing to edge-toolkit core

This guide sets up a machine to build, test and change this repository. The [README](README.md) covers what the
project is and how to run it.

Work through the sections in order. Everything works on Linux, macOS and Windows, and every tool is installed into
the local workspace, so no admin or root privileges are needed. Windows needs a few extra steps, each kept in the
section it belongs to.

## Install mise

Install [`mise`](https://mise.jdx.dev/) at the version the README gives, including the shell integration. It is
needed for all use of this repository.

The `mise` configuration lives under [`.mise/`](.mise/): the always-loaded [`.mise/config.toml`](.mise/config.toml)
holds the Rust/Node tooling and shared tasks, and per-language `.mise/config.<lang>.toml` files are selected via
`MISE_ENV` so a dev can work on one language without installing the others -- e.g.
`MISE_ENV=dart mise install`. CI runs every language; `mise run check-all` (and `install-all`, `test-all`, ...) act
on all of them at once.

### Installing mise on Windows

On Windows this project is set up with [scoop](https://scoop.sh/) as the package manager -- install both
`mise` and `git` through it. `git` is needed for more than version control here: it ships the real `bash.exe`
that the GitHub credential command [depends on](#the-credential-command-on-windows), and `scoop prefix git`
resolves to that install:

```powershell
scoop install mise git
```

mise.exe links the Microsoft VC++ runtime (`vcruntime140.dll`), so it must be present or mise won't start. It's
preinstalled on Windows 10/11 and Server, so you already have it -- only Nano Server omits it, and there the
Docker build installs the [VC++ Redistributable](https://aka.ms/vs/17/release/vc_redist.x64.exe).

## GitHub rate limits

`mise install` downloads many tools from GitHub releases, which are subject to
[GitHub's REST API rate limits](https://docs.github.com/en/rest/using-the-rest-api/rate-limits-for-the-rest-api).
Unauthenticated requests share a low per-IP limit, so installs can fail with
`GitHub rate limit exceeded`. Authenticate with the
[`gh` CLI](https://cli.github.com/) and let `mise` reuse its token -- this raises
the limit and needs no token scopes:

```bash
mise install gh
```

```bash
gh auth login                                        # any method; no scopes needed
mise settings set github.credential_command "gh auth token"
mise token github                                    # verify: should resolve a token
```

If you haven't activated mise's shell integration, use this instead, though it is brittle:

```bash
mise settings set github.credential_command "$(mise which gh) auth token"
```

`gh` often stores the token in the OS keyring rather than in `~/.config/gh/hosts.yml`, so mise's default `hosts.yml`
lookup finds nothing; `credential_command` asks `gh` for the token on demand and works either way. The setting is
written to your global `~/.config/mise/config.toml`, so it stays per-machine and out of the repo.

### The credential command on Windows

On Windows, the `credential_command` set above does not work, and has to be replaced once `gh` is installed and
authenticated. The repo sets `windows_default_inline_shell_args = "bash -euo pipefail -c"` (its tasks need bash),
and mise runs `github.credential_command` **through that inline shell** -- with its own shims stripped from `PATH`,
and ignoring `MISE_BASH_PATH` (that override only applies to task execution, not credential commands). The repo's
only bash is busybox `ash.exe`, reached solely via `MISE_BASH_PATH`, so a literal `bash` is not found: the
credential command fails silently and installs fall back to unauthenticated, rate-limited requests.

Two per-machine settings fix it -- the repo's `bash` inline-shell setting stays untouched:

```powershell
# 1. Put a real bash on PATH via a single scoop shim, pointing at the bash that ships with scoop's git.
#    Nothing else from git lands on PATH, and scoop's shims dir is on PATH and is not stripped by mise.
scoop shim add bash "$(scoop prefix git)\bin\bash.exe"

# 2. Point credential_command at gh's ABSOLUTE path with FORWARD slashes: inside `bash -c` a backslash is
#    an escape character, so a `C:\...` path would be mangled and gh would not be found.
mise settings set github.credential_command "$((mise which gh) -replace '\\','/') auth token"
```

`mise token github` should then resolve a token reported as `source: credential_command`.

## Windows builds

### Shell

On Windows, install the shell before installing the tools:

```bash
mise install http:busybox
```

### Rust targets

mise-powered builds use the `x86_64-pc-windows-gnullvm` Rust target (llvm-mingw), which needs no MSVC toolchain or
Windows SDK on disk. Two opt-in target envs retarget the native build: `MISE_ENV=mingw` switches to
`x86_64-pc-windows-gnu` with the winlibs mingw-w64 GCC toolchain installed by mise, and `MISE_ENV=msvc` switches
to `x86_64-pc-windows-msvc` with a portable MSVC compiler + Windows SDK staged by `mise run prefetch:msvc` -- no
Visual Studio install or admin rights needed for either.

**Known gap on `MISE_ENV=mingw`:** `et-ws-wasi-runner` does not work on `x86_64-pc-windows-gnu`. It instantiates
a WASI component and then aborts while connecting, with tokio reporting no running reactor from inside a host
call, followed by a non-unwinding panic that takes the process down (`Error 0xc0000409`). The default `gnullvm`
target and `MISE_ENV=msvc` both run the same workload cleanly, so use either of those if you need the WASI
runner; the two scenario tests it triggers are skipped on the mingw target alone. Nothing else is known to be
affected -- the mingw lane otherwise runs the full suite, including crates the default Windows target skips.

### et-ws-web-runner on Windows

`et-ws-web-runner` embeds V8 through `deno_core` and the `v8` crate, and cannot build on the gnullvm default
target: there is no `x86_64-pc-windows-gnullvm` `librusty_v8` prebuilt and the gnullvm build path in `rusty_v8`
is still unfinished (open PRs [denoland/rusty_v8#1880](https://github.com/denoland/rusty_v8/pull/1880) and
[#1957](https://github.com/denoland/rusty_v8/pull/1957)). Two mise target envs each give it a working Windows
build: `MISE_ENV=msvc` uses rusty_v8's native prebuilt on `x86_64-pc-windows-msvc`, and `MISE_ENV=mingw` links
that same msvc prebuilt into an `x86_64-pc-windows-gnu` binary (the CRT bridging lives in
`services/ws-web-runner/mingw-shim/` and the crate's `build.rs`).

## Install the tools

Run the preinstall task for your platform -- it configures mise and installs the shared basics (cargo-binstall,
node, the openssl dev files) plus whatever that platform needs -- then install every tool:

```bash
mise run preinstall
mise install-all
```

The `preinstall` task reports any required dependency that is missing, such as the Xcode Command Line Tools on macOS.

### Optionally pre-populating tools from the OCI stores

Most of the tool tree can be restored from this project's per-platform OCI packages on ghcr.io instead of
downloaded tool-by-tool from each upstream:

```bash
mise run pull-mise-tools
```

This pre-populates the local mise data dir from `ghcr.io/edge-toolkit/core/mise-tools/<os>-<arch>`, so the
`mise install` that follows only fills the gaps (the store carries relocatable tools only) and regenerates shims.
This is the same store CI restores from; it needs no authentication.

### Install failures

`mise install` runs tool installs in parallel. If they fail intermittently -- a download race, or a `cargo:` source
build colliding with another -- serialize them with `MISE_JOBS=1`:

```bash
MISE_JOBS=1 mise install-all
```

This is the same workaround both Docker builds bake in, so reach for it first if a local install or build misbehaves.

## Checks and formatting

Use `mise run fmt-all` and `mise run check-all` to run formatters and checkers. [CHECKS.md](CHECKS.md) lists every
check across every `MISE_ENV`, the tool and config each one uses, and every rule those tools are configured with.
