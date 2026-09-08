# syntax=docker/dockerfile:1

# The web-runner image: `et-ws-web-runner` and nothing else.
# The runner is a WebSocket client, not a server. It dials the hub named by WS_SERVER_URL, fetches the module named
# by RUNNER_MODULE from that hub at run time, and executes it on the embedded Deno runtime. So this image serves no
# port and bakes in no module: which module an instance runs is a per-agent decision a deployment makes by setting
# RUNNER_MODULE, exactly as the hub image leaves module provisioning to `et-cli`.
#
# V8 needs no runtime package of its own. `deno_core` links the rusty_v8 static archive into the binary, so the
# runtime stage carries the executable and a trust store and nothing else.
#
# docker build -t et-ws-web-runner -f services/ws-web-runner/Dockerfile .
# docker run --rm -e RUNNER_MODULE=js-math1 -e WS_SERVER_URL=<hub-url> et-ws-web-runner
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

WORKDIR /workspace

COPY . .

# The release build, followed by the extension sources deno loads lazily.
# One instruction rather than two, because the staging step reads the cargo registry that the build populates:
# it can never be cached independently of the build, so splitting them would add a layer and buy nothing.
#
# Roughly twenty deno crates declare `lazy_loaded_js` (deno_telemetry's telemetry.ts, deno_web, deno_node, ...).
# Those sources are meant to be baked into a V8 startup snapshot at build time; this workspace builds no snapshot,
# so each one stays a path -- the compile-time location of the file inside CARGO_HOME -- and is read from disk the
# first time the runtime touches it. That path exists in this stage and not in a slim runtime stage, which is why
# the binary starts, fetches its module, and only then dies on `JsRuntime: No such file or directory`.
# `cp --parents` with absolute sources rebuilds the full path under the staging root, so the COPY in the runtime
# stage restores each file exactly where its registration points. The whole set is a few megabytes of text.
RUN bash <<'EOF'
set -euo pipefail
cargo build --release --locked -p et-ws-web-runner
mkdir -p /staged-ext
src="${CARGO_HOME}/registry/src"
for ext in ts js mjs; do
    find "$src" -type f -path '*/deno_*' -name "*.${ext}" -print0 | xargs -0 -r cp --parents -t /staged-ext
done
EOF

FROM debian:bookworm-slim AS runtime

# ca-certificates supplies the trust store for a wss:// hub and for an https OTLP collector.
# The unprivileged `app` account owns everything the runner writes.
RUN bash <<'EOF'
set -euo pipefail
apt-get update
apt-get install -y --no-install-recommends ca-certificates
apt-get clean
rm -rf /var/lib/apt/lists/* /var/cache/apt/archives/*.deb
useradd --create-home --uid 10001 --user-group app
EOF

WORKDIR /app

COPY --from=builder /workspace/target/release/et-ws-web-runner /usr/local/bin/et-ws-web-runner

# The lazily-loaded extension sources, restored to the absolute paths their registrations name.
# Staged in the builder rather than copied straight from the registry so only these text files land here, not
# the whole vendored source tree.
COPY --from=builder /staged-ext/ /

# /app is the runner's working directory and is owned by the account that runs it.
# The runner itself writes nothing: it resolves the module's `main` field over REST and hands the resulting URL
# straight to the Deno runtime, which loads it without staging anything on disk. The writable cwd is for the
# module -- a guest that opens a file should not fail on a root-owned directory it had no say in.
RUN bash <<'EOF'
set -euo pipefail
chown -R 10001:10001 /app
EOF

# Numeric uid, matching the useradd above, so a host inspecting the image resolves it without our passwd file.
USER 10001

CMD ["et-ws-web-runner"]
