# syntax=docker/dockerfile:1

# The PyO3-runner image: `et-ws-pyo3-runner` plus the CPython it embeds.
# The runner is a WebSocket client, not a server, so this image serves no port. Unlike the WASI runner it does not
# download its module: it imports RUNNER_MODULE from `sys.path` first and only then dials the hub named by
# WS_SERVER_URL, which it uses for the agent connection and for storage. The module therefore has to be present
# locally, and PYO3_PYTHONPATH is how a deployment says where -- its colon-separated entries are prepended to
# `sys.path` before the import.
#
# Nothing is baked in: no module, and deliberately no third-party Python package either. A module that imports
# torch needs torch provisioned beside it, which is a per-cluster decision `et-cli` makes exactly as it does for
# the hub's modules; baking the largest such package into a base image would cost every deployment that never
# uses it.
#
# The interpreter lives at a fixed /opt/cpython in BOTH stages, and that is load-bearing rather than tidiness.
# pyo3 links libpython dynamically, so the finished binary needs the very interpreter it was compiled against --
# same version, same ABI -- present at run time. Pinning one path both stages agree on is what guarantees that,
# and choosing a path with no version in it keeps the pin in `.mise/config.toml` alone rather than copied into a
# directory name here.
#
# The loader is pointed at that directory with ldconfig rather than an rpath, which was tried first and does not
# work. The crate's build script offers pyo3-build-config's lib directory as `-rpath`, but that call returns None
# for this interpreter, so the script takes its documented early return, emits no link argument, and the binary
# comes out with no RPATH or RUNPATH at all -- while still carrying a DT_NEEDED on libpython3.13.so.1.0. The image
# built cleanly and then died on startup with `error while loading shared libraries`. A search path the loader is
# told about directly does not depend on the build script having got there first.
#
# docker build -t et-ws-pyo3-runner -f services/ws-pyo3-runner/Dockerfile .
# docker run --rm -e RUNNER_MODULE=pymath1 -e WS_SERVER_URL=<hub-url> et-ws-pyo3-runner
# A generated deployment fills WS_SERVER_URL in for itself; run by hand, point it at the hub you want.
# Add -e PYO3_PYTHONPATH=/app/modules -v ./modules:/app/modules to supply the module and its packages.

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

# The mise configs come first, on their own layer, so editing source does not reinstall the interpreter.
# Its version is whatever `.mise/config.toml` pins, never a copy of that number kept here.
COPY .miserc.toml .miserc.toml
COPY .mise/ .mise/
RUN bash <<'EOF'
set -euo pipefail
mise trust
mise install python
cp -a "$(mise where python)" /opt/cpython
EOF

# Point pyo3 at the relocated interpreter so it compiles against the same one the runtime stage ships.
# PYO3_PYTHON is the first thing pyo3-build-config consults; without it the build would resolve `python3` from
# PATH, which in this stage is Debian's, and the binary would then carry a DT_NEEDED for that interpreter's
# libpython rather than the pinned one.
#
# `cpython` rather than `python` in the directory name is deliberate, and renaming it back breaks the build.
# The version-drift policy reads any `<tool-name>/<segment>` path as a mise install directory followed by that
# tool's version, so `/opt/python/bin` parses as python pinned to "bin" and fails the Dockerfile checks. A name
# the pattern does not match keeps the check meaningful here rather than carving an exception into it.
ENV PYO3_PYTHON=/opt/cpython/bin/python3

COPY . .

RUN bash <<'EOF'
set -euo pipefail
cargo build --release --locked -p et-ws-pyo3-runner
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

# The whole interpreter tree ships, not just libpython.
# `auto-initialize` starts CPython inside the runner process on first use, and that needs the standard library as
# well as the shared object, so copying out the single library file would leave the interpreter unable to import
# anything. PYTHONHOME names the relocated prefix: with no python executable in the process to derive it from, an
# embedded interpreter otherwise falls back to the prefix compiled into it at build time, which was the install
# path this image moved the tree away from.
COPY --from=builder /opt/cpython /opt/cpython
ENV PYTHONHOME=/opt/cpython

RUN bash <<'EOF'
set -euo pipefail
echo /opt/cpython/lib > /etc/ld.so.conf.d/cpython.conf
ldconfig
EOF

COPY --from=builder /workspace/target/release/et-ws-pyo3-runner /usr/local/bin/et-ws-pyo3-runner

# /app is the runner's working directory and is owned by the account that runs it.
# The runner itself writes nothing here -- the module arrives on PYO3_PYTHONPATH and storage goes to the hub over
# REST -- but an imported module gets a writable cwd, and CPython can drop a __pycache__ beside a mounted source
# tree, so leaving the directory root-owned makes those fail for no reason.
RUN bash <<'EOF'
set -euo pipefail
chown -R 10001:10001 /app
EOF

# Numeric uid, matching the useradd above, so a host inspecting the image resolves it without our passwd file.
USER 10001

CMD ["et-ws-pyo3-runner"]
