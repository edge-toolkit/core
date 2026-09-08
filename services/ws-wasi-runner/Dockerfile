# syntax=docker/dockerfile:1

# The WASI-runner image: `et-ws-wasi-runner` plus the two native libraries its host imports are backed by.
# The runner is a WebSocket client, not a server. It dials the hub named by WS_SERVER_URL, fetches the WASI
# Preview 2 component named by RUNNER_MODULE from that hub at run time, and instantiates it under wasmtime. So this
# image serves no port and bakes in no component: which one an instance runs is a per-agent decision a deployment
# makes by setting RUNNER_MODULE.
#
# Unlike the other two runners this one cannot be a single static binary, because two of the WASI interfaces it
# offers guests are backed by native libraries. `wasi:nn` runs through ONNX Runtime, which ort links dynamically
# (the workspace enables its `copy-dylibs` feature rather than `download-binaries`), so libonnxruntime has to be
# present at run time. `wasi:webgpu` runs through wgpu, which needs a Vulkan loader plus a driver -- mesa's
# lavapipe is the software one, so a container with no GPU still resolves an adapter instead of failing
# `request-adapter`.
# The CUDA execution provider is deliberately not built in: it is opt-in behind the crate's `cuda` feature because
# the prebuilt only exists for some triples, and enabling it here would bind the image to those.
#
# docker build -t et-ws-wasi-runner -f services/ws-wasi-runner/Dockerfile .
# docker run --rm -e RUNNER_MODULE=wasi-graphics-info -e WS_SERVER_URL=<hub-url> et-ws-wasi-runner
# A generated deployment fills WS_SERVER_URL in for itself; run by hand, point it at the hub you want.

FROM rust:1-bookworm AS builder

# clang and cmake are load-bearing rather than conveniences.
# rustls resolves to aws-lc-rs, whose build script drives cmake over the vendored C sources and needs libclang for
# its bindgen pass; the release build fails inside that crate without either. pkg-config is what the remaining
# `-sys` crates probe system libraries with.
RUN bash <<'EOF'
set -euo pipefail
apt-get update
apt-get install -y --no-install-recommends clang cmake pkg-config
apt-get clean
rm -rf /var/lib/apt/lists/* /var/cache/apt/archives/*.deb
EOF

# mise is fetched to a file and checksum-verified, never piped from mise.run into a shell.
# Piping a remote script into an interpreter runs unreviewed code straight off the network, which the repo's
# no-curl-pipe rule bans. The release's SHASUMS256.txt covers every asset, so `--ignore-missing` checks exactly the
# archive downloaded here. Pinned rather than taken from mise's apt repository, which carries only the newest
# version: a floating mise writes lockfile entries an older CI mise then refuses to install.
ARG MISE_VERSION=v2026.9.0
ARG TARGETARCH
ENV MISE_VERSION=${MISE_VERSION}
ENV TARGETARCH=${TARGETARCH}
RUN bash <<'EOF'
set -euo pipefail
case "${TARGETARCH}" in
    amd64) arch=x64 ;;
    arm64) arch=arm64 ;;
    *) echo "unsupported TARGETARCH ${TARGETARCH}" >&2; exit 1 ;;
esac
asset="mise-${MISE_VERSION}-linux-${arch}.tar.gz"
base="https://github.com/jdx/mise/releases/download/${MISE_VERSION}"
cd /tmp
curl -fsSL -o "${asset}" "${base}/${asset}"
curl -fsSL -o SHASUMS256.txt "${base}/SHASUMS256.txt"
sha256sum -c --ignore-missing SHASUMS256.txt
tar -xzf "${asset}"
install -m 0755 /tmp/mise/bin/mise /usr/local/bin/mise
rm -rf /tmp/mise "/tmp/${asset}" /tmp/SHASUMS256.txt
EOF

WORKDIR /workspace

# The mise configs come first, on their own layer, so editing source does not refetch ONNX Runtime.
# Its version is whatever `.mise/config.toml` pins, never a copy of that number kept here; the install is then
# lifted to a fixed path so neither the ENV below nor the runtime stage has to name the version at all.
COPY .miserc.toml .miserc.toml
COPY .mise/ .mise/
RUN bash <<'EOF'
set -euo pipefail
mise trust
mise install github:microsoft/onnxruntime
cp -a "$(mise where github:microsoft/onnxruntime)" /opt/onnxruntime
EOF

# ort is told where ONNX Runtime already is instead of downloading its own copy at build time.
ENV ORT_LIB_LOCATION=/opt/onnxruntime

COPY . .

RUN bash <<'EOF'
set -euo pipefail
cargo build --release --locked -p et-ws-wasi-runner
EOF

FROM debian:bookworm-slim AS runtime

# ca-certificates supplies the trust store for a wss:// hub and for an https OTLP collector.
# libvulkan1 is the loader and mesa-vulkan-drivers carries lavapipe, the software rasteriser wgpu falls back to
# when the container has no GPU. The unprivileged `app` account owns everything the runner writes.
RUN bash <<'EOF'
set -euo pipefail
apt-get update
apt-get install -y --no-install-recommends ca-certificates libvulkan1 mesa-vulkan-drivers
apt-get clean
rm -rf /var/lib/apt/lists/* /var/cache/apt/archives/*.deb
useradd --create-home --uid 10001 --user-group app
EOF

WORKDIR /app

# ONNX Runtime goes on the loader's search path rather than beside the binary.
# ort's `copy-dylibs` also drops a copy next to the executable in the target directory, which resolves through an
# $ORIGIN rpath -- but that only holds while the binary keeps its build-time neighbours, and this stage moves it to
# /usr/local/bin alone. A SONAME lookup through ldconfig does not care where either file ended up.
COPY --from=builder /opt/onnxruntime/lib /usr/local/lib/onnxruntime
RUN bash <<'EOF'
set -euo pipefail
echo /usr/local/lib/onnxruntime > /etc/ld.so.conf.d/onnxruntime.conf
ldconfig
EOF

COPY --from=builder /workspace/target/release/et-ws-wasi-runner /usr/local/bin/et-ws-wasi-runner

# /app is the runner's working directory and is owned by the account that runs it.
# The runner itself writes nothing: the component is collected into memory and instantiated from those bytes, and
# a `wasi:nn` guest's weights arrive the same way through the storage host import. The writable cwd is for the
# guest -- a component granted a preopen should not fail on a root-owned directory it had no say in.
RUN bash <<'EOF'
set -euo pipefail
chown -R 10001:10001 /app
EOF

# Numeric uid, matching the useradd above, so a host inspecting the image resolves it without our passwd file.
USER 10001

CMD ["et-ws-wasi-runner"]
