# Build, test, and serve edge-toolkit from a clean, minimal Ubuntu, split into
# stages so each can be cached and targeted independently:
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
# A plain build produces the SERVER image (final stage): a release et-ws-server,
# served automatically. A GitHub token avoids mise's 60-req/hr anonymous limit
# during install-all:
# DOCKER_BUILDKIT=1 docker build --secret id=gh_token,env=GITHUB_TOKEN -t edge-toolkit .
# docker run --rm -p 8080:8080 edge-toolkit          # serves; open http://localhost:8080
# (drop --secret to build tokenless; install-all may then hit rate limits)
#
# To run the verification suite, target the non-final `test` stage and pass the
# host GPU (`docker build` can't attach one). The stage bundles mesa-vulkan-
# drivers, so the wgpu test gets a real Intel/AMD GPU via the DRI node (or a
# software fallback if none is passed):
# docker build --target test -t edge-toolkit-test .
# docker run --rm --device /dev/dri edge-toolkit-test       # Intel/AMD (verified)
# NVIDIA via `--gpus all` is wired but UNVERIFIED (its in-container Vulkan ICD
# doesn't initialize yet) -- prefer a DRI device.

# --- build-minimal: mise + the always-loaded toolchain (config.toml only). ---
# Copies just .mise/config.toml + installs the default tools, so this layer is
# reused until the always-loaded toolset changes -- not when a guest config does.
FROM ubuntu:24.04 AS build-minimal

# Universal prereqs a typical dev box already has; everything else is mise's job.
# gcc, g++, libc6-dev and make are the C/C++ toolchain rustc links through (`cc`)
# and that C/C++ `-sys` crates build with (make for build scripts that shell out
# to it) -- leaner than build-essential, which also pulls dpkg-dev + perl.
# curl + ca-certificates fetch the mise installer and tool downloads; git is for
# cargo + repo operations; gnupg (gpg + gpg-agent + dirmngr) lets mise verify
# downloads (bare `gpg` lacks the agent/dirmngr it needs); xz-utils, unzip and
# bzip2 unpack mise's tool archives (e.g. the pyodide .tar.bz2). libicu74 is .NET
# runtime ICU for the dotnet-data1 module -- without it the dotnet CLI
# FailFast-aborts at startup ("Couldn't find a valid ICU package installed on the
# system"; minimal Ubuntu ships no ICU). The "74" tracks the Ubuntu base
# (74 = 24.04) -- bump it alongside the FROM line; .NET needs libicu on minimal
# systems (else set System.Globalization.Invariant=true).
# Vulkan for the wgpu test (libvulkan1 + mesa-vulkan-drivers) is installed in the
# test stage, not here.
RUN apt-get update \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
        bzip2 ca-certificates curl g++ gcc git gnupg libc6-dev libicu74 make unzip xz-utils \
    && rm -rf /var/lib/apt/lists/*

# Install mise and put it + its shims on PATH; in a non-interactive build that's
# the equivalent of the shell integration -- every `mise` / `mise run` below
# then resolves the workspace tools.
RUN curl -fsSL https://mise.run | sh
ENV PATH="/root/.local/bin:/root/.local/share/mise/shims:${PATH}"

WORKDIR /workspace
# Only the always-loaded config is needed for the default tools; the
# guest-language configs come in the build stage below. setup-linux is a repo
# task, so the config has to be copied + trusted before it can run.
COPY .mise/config.toml .mise/config.toml

RUN mise trust

# Preinstall via the shared setup-linux task (the same a Linux workstation runs):
# its setup-all base enables experimental + cargo.binstall and installs
# cargo-binstall, node and conda:openssl; then `mise install` adds the rest of
# the always-loaded tools. A GitHub token (if provided) lifts the anonymous rate
# limit for the release fetches.
RUN --mount=type=secret,id=gh_token,required=false \
    GITHUB_TOKEN="$(cat /run/secrets/gh_token 2>/dev/null || true)" \
    sh -c 'mise run setup-linux && mise install'

# --- build: add the guest-language toolchains (config.<lang>.toml). ---
# install-all == MISE_ENV="$ALL_LANGS" mise install; the always-loaded tools are
# already installed by build-minimal, so this adds dart/dotnet/java/zig/etc.
FROM build-minimal AS build
COPY .mise/ .mise/
RUN mise trust
ENV MISE_ENV="dart,dotnet,java,python,rust,zig"
RUN --mount=type=secret,id=gh_token,required=false \
    GITHUB_TOKEN="$(cat /run/secrets/gh_token 2>/dev/null || true)" \
    mise install-all

# --- prefetch: download all dependencies + ONNX models. ---
# The full source is needed from here on (module builds, cargo fetch, pnpm).
FROM build AS prefetch
COPY . .
RUN --mount=type=secret,id=gh_token,required=false \
    GITHUB_TOKEN="$(cat /run/secrets/gh_token 2>/dev/null || true)" \
    mise run prefetch

# --- precompile: build the WASM/JS modules (needed by test and server). ---
# `&& rm -rf target` in the SAME layer: build-modules leaves multi-GB cargo
# intermediates in target/, but the module outputs live in each module's pkg/.
# Dropping target/ here keeps it out of this layer and the stages built on it;
# test and server recompile only what they need.
FROM prefetch AS precompile
RUN mise run build-modules && rm -rf target/

# --- test: the full suite (Rust + web runner + every guest language). ---
# Compiled AND run at `docker run` time (precompile keeps no target/), so the
# multi-GB debug test binaries never bake into a layer -- they live in the
# ephemeral container and vanish when it exits. The wgpu compute test needs
# Vulkan: libvulkan1 (the loader) + mesa-vulkan-drivers give a real Intel/AMD GPU
# when the host DRI node is passed with `--device /dev/dri`, else a CPU (lavapipe)
# fallback so the suite still runs. (NVIDIA's `--gpus` Vulkan path doesn't
# initialize in a container.) Both live here, not the build stage, to keep that
# layer cached.
# docker build --target test -t edge-toolkit-test .
# docker run --rm --device /dev/dri edge-toolkit-test       # Intel/AMD (verified)
# NVIDIA via `--gpus all` is wired (NVIDIA_DRIVER_CAPABILITIES=all below, needs
# the NVIDIA Container Toolkit) but UNVERIFIED -- its in-container Vulkan ICD
# doesn't initialize yet, so prefer a DRI device for now.
FROM precompile AS test
ENV NVIDIA_VISIBLE_DEVICES=all NVIDIA_DRIVER_CAPABILITIES=all
RUN apt-get update \
    && apt-get install -y --no-install-recommends libvulkan1 mesa-vulkan-drivers \
    && rm -rf /var/lib/apt/lists/*
CMD ["mise", "run", "test"]

# --- server: release build of et-ws-server, the default image (final stage). ---
# A plain `docker build` produces this. The release binary is copied out and
# target/ dropped in the SAME layer so the build intermediates don't bloat the
# image; the binary finds its libs via baked rpaths and serves each module from
# its pkg/ (none of which live in target/). mise stays on PATH and MISE_ENV is
# set, so the server's `mise where` module-path lookups resolve.
# docker run --rm -p 8080:8080 edge-toolkit   # then open http://localhost:8080
FROM precompile AS server
RUN mise exec -- cargo build --release -p et-ws-server \
    && cp target/release/et-ws-server /usr/local/bin/et-ws-server \
    && rm -rf target/
EXPOSE 8080 8443
CMD ["et-ws-server"]
