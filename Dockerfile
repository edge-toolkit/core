# syntax=docker/dockerfile:1

# `# syntax=...` opts the Docker engine into the latest dockerfile frontend.
# That frontend is the parser that understands `RUN bash <<'EOF'` heredocs +
# `--mount=` flags. Without it the engine falls back to its built-in parser,
# which on older Docker versions doesn't recognise heredoc and concatenates
# the body into a single command string -- the heredoc then gets handed to
# `/bin/sh -c` verbatim, tripping CI with `Illegal option -o pipefail`. Keep
# the `# syntax=` directive as line 1 of the file.

# Build, test, and serve edge-toolkit from a clean, minimal Ubuntu.
# It is split into stages so each can be cached and targeted independently:
#
# build-minimal  mise + the always-loaded toolchain (.mise/config.toml only)
# build          + the guest-language toolchains (.mise/config.<lang>.toml)
# prefetch       download all dependencies + ONNX models
# precompile     build the WASM/JS modules (drops target/ to stay slim)
# test           compile + run the full suite (ephemeral, at `docker run`; needs a GPU)
# server         release build of et-ws-server, served by default (final stage)
#
# Each stage `FROM`s the previous, so installed tools, downloaded deps, and the
# built module pkg/ carry forward. Stop early with `--target`.
#
# The build stages run the mise setup verbatim (install mise -> configure ->
# install conda:openssl -> install), split so build-minimal (the always-loaded
# tools) caches separately from the guest languages (build). The OpenObserve
# (`o2`/`open-o2`) and `ws-server` runtime services are intentionally skipped --
# they aren't build/test steps.
#
# It also catches setup drift: a missing or wrong step fails the build. Anything
# language-specific must come from mise (the "installed into the local
# workspace" promise); the only apt packages below are universal build prereqs a
# normal dev machine has, so a failure that needs another system lib is itself a
# finding worth documenting.
#
# The server image is the `server` stage: a release et-ws-server, served
# automatically. A GitHub token avoids mise's anonymous GitHub rate limit
# during install-all. `--target server` is explicit because the final stage
# in this file is `check` (built by CI), not server:
# DOCKER_BUILDKIT=1 docker build --target server --secret id=gh_token,env=GITHUB_TOKEN -t edge-toolkit .
#
# docker run --rm -p 8080:8080 edge-toolkit          # serves; open http://localhost:8080
# (drop --secret to build tokenless; install-all may then hit rate limits)
#
# To run the verification suite, target the `test` stage and pass the host
# GPU (`docker build` can't attach one). The stage bundles mesa-vulkan-
# drivers, so the wgpu test gets a real Intel/AMD GPU via the DRI node (or a
# software fallback if none is passed):
# DOCKER_BUILDKIT=1 docker build --target test --secret id=gh_token,env=GITHUB_TOKEN -t edge-toolkit-test .
# docker run --rm --device /dev/dri edge-toolkit-test       # Intel/AMD (verified)
# NVIDIA via `--gpus all` is wired but UNVERIFIED (its in-container Vulkan ICD
# doesn't initialize yet) -- prefer a DRI device.

# build-minimal: mise + the always-loaded toolchain (config.toml only).
# Copies just .mise/config.toml + installs the default tools, so this layer is
# reused until the always-loaded toolset changes -- not when a guest config does.
# Base image is parameterised so the CI matrix can build Debian + Ubuntu
# variants; bump LIBICU_PKG below alongside BASE_IMAGE.
ARG BASE_IMAGE=ubuntu:24.04
FROM ${BASE_IMAGE} AS build-minimal

# Universal prereqs a typical dev box already has; everything else is mise's job.
# gcc, g++, libc6-dev and make are the C/C++ toolchain rustc links through (`cc`)
# and that C/C++ `-sys` crates build with (make for build scripts that shell out
# to it) -- leaner than build-essential, which also pulls dpkg-dev + perl.
# curl + ca-certificates fetch the mise installer and tool downloads; git is
# for cargo + repo operations; unzip / bzip2 / gzip / tar unpack mise's tool
# archives. `xz` is NOT here -- mise extracts .tar.xz natively via the xz2
# Rust crate (mise/src/file.rs:910,1218), no subprocess. `bash` is explicit:
# every mise task in this repo runs under `shell = "bash -euo pipefail -c"`,
# and Debian/Ubuntu use dash as /bin/sh with bash a separate package
# (Fedora/Azure Linux ship bash as /bin/sh already, so listing it there is a
# no-op).
# Three package lists: COMMON has names that are identical across every
# package manager we hit, so no arm has to repeat them. APT_PACKAGES and
# DNF_PACKAGES carry only the distro-specific extras (apt's `g++` vs dnf's
# `gcc-c++`, etc.); the zypper arm reuses DNF_PACKAGES with a rename map
# for the two names openSUSE spells differently (`kernel-headers` ->
# `linux-glibc-devel`, `libatomic` -> `libatomic1`). ICU (needed by .NET
# runtime for the dotnet-data1 module -- without it the dotnet CLI
# FailFast-aborts at startup with "Couldn't find a valid ICU package
# installed on the system") is named per distro: on apt the runtime pkg
# is libicu<NN> (auto-detected via `apt-cache search`, covers libicu72
# bookworm / libicu74 ubuntu 24.04 / libicu76 trixie / libicu78 ubuntu
# 26.04); on Fedora's dnf and openSUSE's zypper it's the unversioned
# `libicu`; Azure Linux renames it to `icu`. The RUN below picks the
# install command by whichever package manager exists (apt-get / dnf /
# tdnf / zypper) and detects the few RPM distros that need name renames
# via /etc/os-release.
ARG COMMON_PACKAGES="bash binutils bzip2 ca-certificates curl gcc git gzip make tar unzip"
ARG APT_PACKAGES="g++ libc6-dev"
ARG DNF_PACKAGES="gcc-c++ glibc-devel kernel-headers libatomic libicu"
# Promote ARG -> ENV so downstream shell scripts can read the package lists.
# The RUN heredoc below (and `.mise/config.linux.toml`'s preinstall script)
# read them via the shell's environment. The heredoc
# uses a quoted `<<'EOF'` delimiter, which means Docker does NOT substitute
# `$COMMON_PACKAGES` etc. into the body at build time (and, crucially, the
# outer `/bin/sh` doesn't try to perform `$(...)` command substitution before
# bash runs -- which was breaking the libicu lookup: on Fedora the outer
# `/bin/sh` tried to run `apt-cache` to substitute the `libicu=$(apt-cache
# ...)` line before reaching the dnf branch and crashed with `apt-cache:
# command not found`; on Debian/Ubuntu it ran apt-cache BEFORE the script's
# `apt-get update` line, so the cache was stale and the lookup returned
# empty). The ARG -> ENV step is what makes `$COMMON_PACKAGES` resolve at
# bash-eval time inside the quoted heredoc.
ENV COMMON_PACKAGES=${COMMON_PACKAGES}
ENV APT_PACKAGES=${APT_PACKAGES}
ENV DNF_PACKAGES=${DNF_PACKAGES}
# Leaving these package-list variables unquoted below is intentional.
# `$COMMON_PACKAGES` / `$APT_PACKAGES` / `$DNF_PACKAGES` / `$pkgs` are each a
# space-separated list, and we WANT the shell to word-split each one into
# distinct args for apt-get / dnf / tdnf / zypper.
# SC2086 fires on these and that's the false-positive that pragma silences.
# hadolint ignore=SC2086
RUN bash <<'EOF'
set -euo pipefail
if command -v apt-get >/dev/null 2>&1; then
    apt-get update
    # apt-cache pkgnames lists names with a given prefix, one per line.
    # grep -E then narrows to the versioned package (libicu70 jammy /
    # libicu72 bookworm / libicu74 ubuntu 24.04 / libicu76 trixie etc.).
    # `apt-cache search --names-only '^libicu[0-9]+$'` doesn't work here --
    # apt-cache search treats `--names-only` as filter on already-matched
    # records, not as full-name anchor, so the `^...$` regex misses every
    # debian/ubuntu base we tested.
    libicu="$(apt-cache pkgnames libicu | grep -E '^libicu[0-9]+$' | head -1)"
    [ -n "$libicu" ] || { echo "no libicu[0-9]+ package available in this base" >&2; exit 1; }
    DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends $COMMON_PACKAGES $APT_PACKAGES "$libicu"
    apt-get clean
    rm -rf /var/lib/apt/lists/* /var/cache/apt/archives/*.deb
elif command -v dnf >/dev/null 2>&1 || command -v tdnf >/dev/null 2>&1; then
    is_azl=false
    if grep -q '^ID=azurelinux$' /etc/os-release 2>/dev/null; then is_azl=true; fi
    pkgs=""
    for w in $DNF_PACKAGES; do
        if [ "$is_azl" = true ]; then
            case "$w" in
                libicu) w=icu ;;
                libatomic) w=libgcc-atomic ;;
            esac
        fi
        pkgs="$pkgs $w"
    done
    if command -v dnf >/dev/null 2>&1; then
        dnf install -y --allowerasing --setopt=install_weak_deps=False $COMMON_PACKAGES $pkgs
        dnf clean all
    else
        tdnf install -y $COMMON_PACKAGES $pkgs
        tdnf clean all
    fi
elif command -v zypper >/dev/null 2>&1; then
    pkgs=""
    for w in $DNF_PACKAGES; do
        case "$w" in
            kernel-headers) w=linux-glibc-devel ;;
            libatomic) w=libatomic1 ;;
        esac
        pkgs="$pkgs $w"
    done
    zypper --non-interactive --gpg-auto-import-keys refresh
    zypper --non-interactive install --no-recommends $COMMON_PACKAGES $pkgs
    zypper clean --all
else
    echo "no supported package manager (apt-get, dnf, tdnf, or zypper) found" >&2
    exit 1
fi
EOF

# Install mise and put it + its shims on PATH.
# In a non-interactive build that's the equivalent of the shell integration --
# every `mise` / `mise run` below then resolves the workspace tools.
# Pin the mise version: the config templates need Tera v2 (mise >= 2026.7.1), and mise.run honours MISE_VERSION.
# Keep this in lockstep with min_version in .mise/config.toml.
# skipcq: DOK-DL4006
RUN curl -fsSL https://mise.run | MISE_VERSION=v2026.9.0 sh
# Declare HOME explicitly rather than depending on the base image's ENV.
# ubuntu/debian/fedora all set HOME=/root for the root user, but pinning it
# here means the PATH expansion below doesn't silently break against a future
# minimal base that strips the inherited ENV.
ENV HOME=/root
ENV PATH="${HOME}/.local/bin:${HOME}/.local/share/mise/shims:${PATH}"
# cargo-expand is a dev-only macro-debugging tool with no role in image builds.
# So skip it here (this ENV is inherited by every stage below).
ENV MISE_DISABLE_TOOLS=cargo:cargo-expand

WORKDIR /workspace
# The default tools need the always-loaded config plus config.linux.toml.
# config.linux.toml is where preinstall + the Linux env live; .miserc.toml
# (auto_env) auto-loads it; the other guest-language configs come in the build
# stage below. These are repo configs, so they're copied + trusted before mise
# runs.
COPY .miserc.toml .miserc.toml
COPY .mise/config.toml .mise/config.toml
COPY .mise/task-shell.sh .mise/task-shell.sh
# Each config's lockfile rides with it so in-container resolution stays offline.
# Without them, every `latest` pin costs a per-tool api.github.com /releases lookup.
COPY .mise/mise.lock .mise/mise.lock
COPY .mise/config.linux.toml .mise/config.linux.toml
COPY .mise/mise.linux.lock .mise/mise.linux.lock

RUN mise trust

# Preinstall via the shared preinstall task (the same a Linux workstation runs).
# its setup-all base enables experimental + cargo.binstall and installs
# cargo-binstall, node and conda:openssl; then `mise install` adds the rest of
# the always-loaded tools. A GitHub token (if provided) lifts the anonymous rate
# limit for the release fetches. MISE_JOBS=1 serializes the install: the `rust`
# tool is a two-version list (latest + nightly) that otherwise runs two
# rustup-inits at once, racing on the shared rustup binary's self-update (exit 1).
RUN --mount=type=secret,id=gh_token,required=false bash <<'EOF'
set -euo pipefail
GITHUB_TOKEN="$(cat /run/secrets/gh_token 2>/dev/null || true)"
export GITHUB_TOKEN
mise run preinstall
MISE_JOBS=1 mise install
EOF

# build: add the guest-language toolchains (config.<lang>.toml).
# `mise install` honors MISE_ENV (the always-loaded tools are already
# installed by build-minimal, so this adds whichever guests MISE_ENV names
# -- dart/dotnet/java/python/rust/zig in the default). Stage-scoped ARG
# with a default; override via `--build-arg MISE_ENV=...` to widen or
# narrow the language set without touching this file (the docker-linux.yaml
# workflow passes its workflow-level MISE_ENV through).
FROM build-minimal AS build
COPY .mise/ .mise/
RUN mise trust
ARG MISE_ENV=dart,dotnet,java,js,kotlin,python,r,rust,zig
ENV MISE_ENV=${MISE_ENV}
RUN --mount=type=secret,id=gh_token,required=false bash <<'EOF'
set -euo pipefail
GITHUB_TOKEN="$(cat /run/secrets/gh_token 2>/dev/null || true)"
export GITHUB_TOKEN
mise install
EOF

# prefetch: download all dependencies + ONNX models.
# The full source is needed from here on (module builds, cargo fetch, pnpm).
FROM build AS prefetch
COPY . .
RUN --mount=type=secret,id=gh_token,required=false bash <<'EOF'
set -euo pipefail
GITHUB_TOKEN="$(cat /run/secrets/gh_token 2>/dev/null || true)"
export GITHUB_TOKEN
mise run prefetch
EOF

# precompile: build the WASM/JS modules (needed by test and server).
# `&& rm -rf target` in the SAME layer: build-modules leaves multi-GB cargo
# intermediates in target/, but the module outputs live in each module's pkg/.
# Dropping target/ here keeps it out of this layer and the stages built on it;
# test and server recompile only what they need.
FROM prefetch AS precompile
RUN mise run build-modules && rm -rf target/

# test: the full suite (Rust + web runner + every guest language).
# Compiled AND run at `docker run` time (precompile keeps no target/), so the
# multi-GB debug test binaries never bake into a layer -- they live in the
# ephemeral container and vanish when it exits. The wgpu compute test needs
# Vulkan: libvulkan1 (the loader) + mesa-vulkan-drivers give a real Intel/AMD GPU
# when the host DRI node is passed with `--device /dev/dri`, else a CPU (lavapipe)
# fallback so the suite still runs. (NVIDIA's `--gpus` Vulkan path doesn't
# initialize in a container.) Both live here, not the build stage, to keep that
# layer cached.
# DOCKER_BUILDKIT=1 docker build --target test --secret id=gh_token,env=GITHUB_TOKEN -t edge-toolkit-test .
# docker run --rm --device /dev/dri edge-toolkit-test       # Intel/AMD (verified)
# NVIDIA via `--gpus all` is wired (NVIDIA_DRIVER_CAPABILITIES=all below, needs
# the NVIDIA Container Toolkit) but UNVERIFIED -- its in-container Vulkan ICD
# doesn't initialize yet, so prefer a DRI device for now.
FROM precompile AS test
ENV NVIDIA_VISIBLE_DEVICES=all NVIDIA_DRIVER_CAPABILITIES=all
# Force the Vulkan loader to consider lavapipe (CPU software renderer) only.
# Without it, distros whose `mesa-vulkan-drivers` package also ships a
# PowerVR ICD (notably Azure Linux base/core, where `powervr_mesa_icd.json`
# is staged alongside `lvp_icd.x86_64.json`) try the PVR ICD first; on a
# GPU-less CI runner that ICD fails to enumerate DRM devices, blocking
# adapter discovery before lavapipe gets reached, and the wasi-graphics-info
# test crashes verbatim as:
# wgpu_hal::vulkan::instance: GENERAL [pvr_device.c (0x0)]
# Failed to enumerate drm devices (errno 2: No such file or directory)
# (VK_ERROR_INITIALIZATION_FAILED)
# ...
# Error: ...WebgpuError(... message='no GPU adapter available')
# The value is a comma-separated list of fnmatch globs matched against the
# ICD JSON filename (no implicit substring match -- a bare `lvp_icd` rejected
# every driver on ubuntu:24.04 with "Driver 'lvp_icd.json' ignored because not
# selected by env var 'VK_LOADER_DRIVERS_SELECT'"). The glob below catches
# both `lvp_icd.json` (Debian/Ubuntu/Fedora) and `lvp_icd.x86_64.json`
# (Azure Linux). When the docker host DOES pass `--device /dev/dri`, lvp is
# still a valid CPU fallback alongside the real GPU's ICD; the lvp glob
# selects the software path either way.
ENV VK_LOADER_DRIVERS_SELECT=lvp_icd*
# Install the Vulkan runtime + lavapipe (CPU/software) driver per package manager.
# Debian/Ubuntu: libvulkan1 + mesa-vulkan-drivers; Fedora/AL2023:
# vulkan-loader + mesa-vulkan-drivers; Azure Linux: same as Fedora;
# openSUSE: libvulkan1 + libvulkan_lvp (Mesa ships the lavapipe driver as
# a separate `libvulkan_lvp` package on SUSE since the 2022 default-pkg
# split that stopped pulling it in alongside hardware drivers).
RUN bash <<'EOF'
set -euo pipefail
if command -v apt-get >/dev/null 2>&1; then
    apt-get update
    apt-get install -y --no-install-recommends libvulkan1 mesa-vulkan-drivers
    apt-get clean
    rm -rf /var/lib/apt/lists/* /var/cache/apt/archives/*.deb
elif command -v dnf >/dev/null 2>&1; then
    dnf install -y --setopt=install_weak_deps=False vulkan-loader mesa-vulkan-drivers
    dnf clean all
elif command -v tdnf >/dev/null 2>&1; then
    tdnf install -y vulkan-loader mesa-vulkan-drivers
    tdnf clean all
elif command -v zypper >/dev/null 2>&1; then
    zypper --non-interactive install --no-recommends libvulkan1 libvulkan_lvp
    zypper clean --all
else
    echo "no supported package manager for libvulkan1/mesa-vulkan-drivers" >&2
    exit 1
fi
EOF

CMD ["mise", "run", "test"]

# server: release build of et-ws-server.
# Build with `docker build --target server`. The release binary is copied out
# and target/ dropped in the SAME layer so the build intermediates don't bloat
# the image; the binary finds its libs via baked rpaths and serves each module
# from its pkg/ (none of which live in target/). mise stays on PATH and
# MISE_ENV is set, so the server's `mise where` module-path lookups resolve.
# docker run --rm -p 8080:8080 edge-toolkit   # then open http://localhost:8080
FROM precompile AS server
RUN bash <<'EOF'
set -euo pipefail
mise exec -- cargo build --release -p et-ws-server
cp target/release/et-ws-server /usr/local/bin/et-ws-server
rm -rf target/
EOF

EXPOSE 8080 8443
CMD ["et-ws-server"]

# check: full `mise run check` against the in-image source.
# `.git/` and `Dockerfile*` are excluded from main .dockerignore (the earlier
# stages never see them, so they don't bloat). Both come in here only, via a
# named build context the GHA workflow pre-stages at target/check-ctx (with
# `cp -r .git Dockerfile Dockerfile.nanoserver target/check-ctx/`) and passes
# as `--build-context extras=target/check-ctx`. `COPY --from=extras . ./`
# lands .git/ + Dockerfile* in the workspace. With .git/ on disk the git-
# using checks (action-validator, hadolint via `git ls-files`,
# conftest-check-toml, dockerignore-check, gen-specs-check, verification-check)
# resolve normally. This stage exists only so docker-linux.yaml can run the
# full `mise run check` against each matrix base. Being last in this file
# means `docker build` with no `--target` builds this stage by default --
# README invocations use explicit `--target server`/`--target test` so
# they're unaffected.
FROM precompile AS check
# `extras` is a docker --build-context name, not a stage alias.
# It is passed by docker-linux.yaml; hadolint can't tell the two apart.
# hadolint ignore=DL3022
COPY --from=extras . ./
CMD ["mise", "run", "check"]
