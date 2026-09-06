# syntax=docker/dockerfile:1

# The `# syntax=` directive opts the engine into the BuildKit frontend and has to stay on line 1.
# That frontend is the parser which understands `RUN bash <<'EOF'` heredocs. Without it the engine falls back to
# its built-in parser, which hands the heredoc body to `/bin/sh -c` verbatim -- and the dash that Debian ships as
# /bin/sh then aborts on the first body line with `Illegal option -o pipefail`.

# The hub image: et-ws-server plus only what the hub UI itself cannot start without.
# That is the `et-ws-server-static` root module (the page served at `/`) and the `et-ws-wasm-agent` browser client
# that page imports from `/modules/et-ws-wasm-agent/`. No workflow module, no model weights and no npm runtime
# dependency is baked in: which of those a deployment serves is a per-cluster decision, so `et-cli` provisions them
# on top of this image rather than every image carrying every module.
#
# MODULES_PATHS is deliberately left unset. Unset, the server falls back to `default_modules_folders()`, which
# resolves each default against the working directory -- `/app` in this image -- so the two bundled paths resolve
# and every remaining default is an absent directory the module scan skips. A bare `docker run` therefore serves
# the UI with no configuration at all, and a cluster deployment sets MODULES_PATHS to name what it provisioned.
#
# docker build -t et-ws-server -f services/ws-server/Dockerfile .
# docker run --rm -p 8080:8080 et-ws-server        # then open http://localhost:8080

FROM rust:1-bookworm AS builder

# clang and cmake are load-bearing here rather than conveniences.
# rustls resolves to aws-lc-rs, and the aws-lc-sys build script drives cmake over the vendored C sources plus
# libclang for its bindgen pass; the release build fails inside that crate without either. pkg-config is what the
# remaining `-sys` crates probe system libraries with. Two packages this image used to install are gone:
# protobuf-compiler (opentelemetry-proto ships pre-generated prost types, so no protoc ever runs) and npm/pnpm,
# which only fetched the onnxruntime-web, pyodide and stats-gl bundles that are no longer part of the hub.
RUN bash <<'EOF'
set -euo pipefail
apt-get update
apt-get install -y --no-install-recommends clang cmake pkg-config
apt-get clean
rm -rf /var/lib/apt/lists/* /var/cache/apt/archives/*.deb
EOF

# wasm-pack builds the browser client; keep this in step with the `aqua:rustwasm/wasm-pack` pin in .mise/mise.lock.
ARG WASM_PACK_VERSION=0.15.0
# TARGETARCH is an automatic BuildKit build ARG, and re-declaring it is what brings it into this stage.
ARG TARGETARCH
# Promote both ARGs to ENV so the quoted heredoc below can read them.
# A body opened with a quoted delimiter (`<<'EOF'`) is expanded by bash at run time, not by the frontend at build
# time, so an ARG reaches it only through the environment.
ENV WASM_PACK_VERSION=${WASM_PACK_VERSION}
ENV TARGETARCH=${TARGETARCH}
RUN bash <<'EOF'
set -euo pipefail
case "${TARGETARCH}" in
    amd64) arch=x86_64 ;;
    arm64) arch=aarch64 ;;
    *) echo "unsupported TARGETARCH ${TARGETARCH}: wasm-pack ships x86_64 and aarch64 only" >&2; exit 1 ;;
esac
release="wasm-pack-v${WASM_PACK_VERSION}-${arch}-unknown-linux-musl"
url="https://github.com/wasm-bindgen/wasm-pack/releases/download/v${WASM_PACK_VERSION}/${release}.tar.gz"
curl -fsSL "${url}" | tar -xzf - -C /usr/local/bin --strip-components=1 "${release}/wasm-pack"
rustup target add wasm32-unknown-unknown
EOF

WORKDIR /workspace

COPY . .

# The wasm flags mirror CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUSTFLAGS in .mise/config.toml.
# That is what `mise run build-ws-wasm-agent` builds the agent with on a workstation, so the served artifact is the
# same one either way. They are split across two ARGs purely so the composed ENV fits the line limit.
ARG WASM_TARGET_CPU="-C target-cpu=mvp"
ARG WASM_FEATURES="+mutable-globals,+sign-ext,+nontrapping-fptoint,+reference-types"
ENV CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUSTFLAGS="${WASM_TARGET_CPU} -C target-feature=${WASM_FEATURES}"

RUN bash <<'EOF'
set -euo pipefail
cargo build --release --locked -p et-ws-server
EOF

RUN bash <<'EOF'
set -euo pipefail
cd services/ws-wasm-agent
wasm-pack build . --target web
EOF

FROM debian:bookworm-slim AS runtime

# ca-certificates supplies the trust store the OTLP exporter needs for an https collector.
# The unprivileged `app` account owns everything the server writes.
RUN bash <<'EOF'
set -euo pipefail
apt-get update
apt-get install -y --no-install-recommends ca-certificates
apt-get clean
rm -rf /var/lib/apt/lists/* /var/cache/apt/archives/*.deb
useradd --create-home --uid 10001 --user-group app
EOF

WORKDIR /app

COPY --from=builder /workspace/target/release/et-ws-server /usr/local/bin/et-ws-server
COPY --from=builder --chown=10001:10001 /workspace/services/ws-server/static ./services/ws-server/static
COPY --from=builder --chown=10001:10001 /workspace/services/ws-wasm-agent/pkg ./services/ws-wasm-agent/pkg

# /app has to be writable by the server, not just readable.
# On first start the server generates a self-signed localhost certificate and writes cert.pem plus key.pem into
# its working directory, so a root-owned /app aborts the boot outright. `storage` is the directory the default
# STORAGE_URL (file:///app/services/ws-server/storage/) resolves to; `/app/storage` is where a deployment that
# overrides STORAGE_URL -- as the generated compose stack does -- mounts its volume, and pre-creating it with the
# right owner is what stops Docker from materialising that mount point as root.
RUN bash <<'EOF'
set -euo pipefail
mkdir -p /app/storage /app/services/ws-server/storage
chown -R 10001:10001 /app
EOF

# Numeric uid, matching the useradd above, so a host inspecting the image resolves it without our passwd file.
USER 10001

EXPOSE 8080 8443

CMD ["et-ws-server"]
