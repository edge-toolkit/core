use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::Path;

use fs_err as fs;

use crate::error::CliError;
use crate::input::{ArtifactSource, ClusterInput};
use crate::{IMAGE_REGISTRY, ModuleSource, cluster_module_names, module_registry, resolve_cluster_modules};

/// Build context path of the hub image's Dockerfile, which the scenario image layers on top of.
const HUB_DOCKERFILE: &str = "services/ws-server/Dockerfile";

/// Modules the hub image already carries at these paths, which a scenario image layered on it does not copy again.
///
/// A module that depends on one of them still resolves it, so what it declares in turn is staged as usual; only the
/// module's own files are skipped.
const HUB_IMAGE_MODULES: [&str; 2] = ["/app/services/ws-server/static", "/app/services/ws-wasm-agent"];

/// Guest languages the dependency stage loads so every `[tools]` pin is in scope.
///
/// A mise tool is only installable when the config declaring it is loaded, and the packages modules depend on are
/// spread across several guest configs (`npm:onnxruntime-web` in js, `http:pyodide` in python). Declaring the full set
/// costs nothing -- only the tools named on the `mise install` line are actually fetched.
const DEPS_MISE_ENV: &str = "dart,dotnet,java,js,kotlin,python,r,rust,zig";

/// Where the scenario image's modules come from, split by what each source needs from the build.
///
/// Each list is sorted out of resolution order, which moves a module whenever an unrelated one gains a dependency, so
/// every list the image is rendered from stays put in a committed file: `SCENARIO_TOOLS`, the staging steps, the COPYs.
#[derive(Default)]
struct ImageSources {
    /// This repository's modules, copied from the checkout: `(repo path, served path)`.
    repo_paths: Vec<(String, String)>,
    /// This repository's modules, fetched as the releases a published scenario consumes: `(package, served path)`.
    released: Vec<(String, String)>,
    /// Packages a mise tool provisions, staged from the repository's own pins: `(tool, package, served path)`.
    mise_tools: Vec<(String, String, String)>,
    /// The scenario's own `module_paths:` directories: `(build context, path in it, served path)`.
    scenario_dirs: Vec<(String, String, String)>,
}

/// What building a scenario's image asks of whoever runs the build.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub(crate) struct ImageNeeds {
    /// Whether the build context has to be this repository's checkout rather than the deployment directory.
    pub repository: bool,
    /// Whether the build fetches released modules, and so needs the `gh_token` secret GitHub Packages demands.
    pub registry_token: bool,
}

/// Sort the cluster's modules by where the scenario image gets each one.
///
/// A published scenario takes this repository's modules from their releases, the way every other artifact it names is
/// a release, so its image can be built from the deployment directory alone. A local one copies the modules it was
/// built against out of the checkout.
fn image_sources(cluster: &ClusterInput) -> Result<ImageSources, CliError> {
    let project_root = edge_toolkit::config::get_project_root();
    let ws_server_dir = project_root.join("services/ws-server");
    let registry = module_registry(&project_root, &ws_server_dir, &cluster.module_paths);
    let published = matches!(cluster.artifact_source, ArtifactSource::Published);

    let mut sources = ImageSources::default();
    for entry in resolve_cluster_modules(&registry, &cluster_module_names(cluster))? {
        let docker_path = entry.docker_path;
        match entry.source {
            ModuleSource::Repo(_) if HUB_IMAGE_MODULES.contains(&docker_path.as_str()) => {}
            ModuleSource::Repo(_) if published => sources
                .released
                .push((entry.package_name.unwrap_or_default(), docker_path)),
            ModuleSource::Repo(repo_path) => sources.repo_paths.push((repo_path, docker_path)),
            ModuleSource::MiseTool { tool, package } => sources.mise_tools.push((tool, package, docker_path)),
            ModuleSource::Scenario {
                context, context_path, ..
            } => sources.scenario_dirs.push((context, context_path, docker_path)),
        }
    }
    sources.repo_paths.sort();
    sources.released.sort();
    sources.mise_tools.sort();
    sources.scenario_dirs.sort();
    Ok(sources)
}

/// What building this cluster's scenario image needs: the repository as its context, a registry token, or neither.
///
/// A local scenario is built from the checkout it was generated in, whatever its modules. A published one needs the
/// repository only for a mise-provisioned package, which is staged from the repository's own `.mise/` pins -- so a
/// published scenario whose modules pull in such a package still builds from a checkout.
pub(crate) fn scenario_image_needs(cluster: &ClusterInput) -> Result<ImageNeeds, CliError> {
    Ok(needs_of(cluster, &image_sources(cluster)?))
}

/// [`scenario_image_needs`] for sources already resolved.
const fn needs_of(cluster: &ClusterInput, sources: &ImageSources) -> ImageNeeds {
    ImageNeeds {
        repository: matches!(cluster.artifact_source, ArtifactSource::Local) || !sources.mise_tools.is_empty(),
        registry_token: !sources.released.is_empty(),
    }
}

/// The ignore file for an image built from its own deployment directory, which copies nothing out of its context.
///
/// Everything is excluded, so the build is sent nothing at all -- the directory's `secrets.env` included.
fn render_empty_context_dockerignore(cluster_name: &str) -> String {
    format!(
        concat!(
            "# AUTO-GENERATED by `et-cli` for the `{name}` scenario -- do not edit.\n",
            "# Regenerate with `mise run regen-verification`.\n",
            "#\n",
            "# The Dockerfile beside this file copies nothing from its build context, so the context is empty.\n",
            "*\n",
        ),
        name = cluster_name,
    )
}

pub fn generate_scenario_image(cluster: &ClusterInput, output_dir: &Path) -> Result<(), CliError> {
    let project_root = edge_toolkit::config::get_project_root();
    let sources = image_sources(cluster)?;

    // Trimmed to exactly one trailing newline, because each section appends its own separating blank line and whichever
    // section ends up last therefore leaves one behind. A scenario with no modules at all has no `COPY` sections, so
    // the separator after the label is the end of the file.
    let dockerfile = render_dockerfile(
        &cluster.cluster_name,
        matches!(cluster.artifact_source, ArtifactSource::Published),
        &sources,
    );
    fs::write(output_dir.join("Dockerfile"), format!("{}\n", dockerfile.trim_end()))?;
    let dockerignore = if needs_of(cluster, &sources).repository {
        render_dockerignore(&project_root, &cluster.cluster_name, &sources.repo_paths)?
    } else {
        render_empty_context_dockerignore(&cluster.cluster_name)
    };
    fs::write(output_dir.join("Dockerfile.dockerignore"), dockerignore)?;

    Ok(())
}

/// A path as one operand of a JSON-array `COPY`, which it has to be once the path is not this repository's own.
///
/// A `module_paths:` directory is named by whoever laid out that tree, so it can hold a space -- which the plain form
/// splits into two operands -- or a quote or backslash, which JSON escapes. A `$` is escaped for the Dockerfile itself,
/// since variable substitution still runs on each operand of the JSON form.
fn dockerfile_word(path: &str) -> String {
    serde_json::Value::String(path.replace('$', "\\$")).to_string()
}

/// Render the scenario Dockerfile's opening comment, which says what the image is and where its modules come from.
///
/// A published image copies nothing out of a checkout, so the ignore file that re-admits built artifacts has no part in
/// it; what it says instead is where its modules come from.
fn render_dockerfile_header(cluster_name: &str, published: bool) -> String {
    let mut out = String::default();
    out.push_str("# syntax=docker/dockerfile:1\n\n");
    let sourcing = if published {
        concat!(
            "# This scenario consumes the project's releases, so its modules are fetched from the registry they are\n",
            "# published to rather than copied from a checkout, and the image builds from the deployment directory\n",
            "# alone.\n",
        )
    } else {
        concat!(
            "# The sibling `Dockerfile.dockerignore` is what makes the COPYs below possible. The repository's root\n",
            "# `.dockerignore` drops every `*.wasm` and `*.onnx` so the CI images built from `./Dockerfile` stay\n",
            "# small; BuildKit uses this Dockerfile's own ignore file instead, which re-admits the built `pkg/`\n",
            "# directories and nothing else.\n",
        )
    };
    let _write_result = writeln!(
        out,
        concat!(
            "# AUTO-GENERATED by `et-cli` for the `{name}` scenario -- do not edit.\n",
            "# Regenerate with `mise run regen-verification`.\n",
            "#\n",
            "# The hub image ({hub}) carries no workflow modules at all, because which\n",
            "# modules a deployment serves is a per-cluster decision. This image is that decision made concrete: it\n",
            "# stages exactly the modules this cluster's agents declare, plus the packages those modules depend on,\n",
            "# into the `/app` paths the hub already scans.\n",
            "#\n",
            "{sourcing}",
        ),
        name = cluster_name,
        hub = HUB_DOCKERFILE,
        sourcing = sourcing,
    );
    out
}

/// Render the stage the scenario image is built on: the released hub when published, the one built beside it otherwise.
#[expect(
    clippy::single_call_fn,
    reason = "the base half of render_dockerfile, split out to keep it within its length budget"
)]
fn render_hub_base(published: bool) -> String {
    if published {
        return format!(
            concat!(
                "# The released hub image, which this scenario layers its modules onto.\n",
                "# `latest` because that is the one tag the hub is published under: it is assembled from each\n",
                "# architecture's build, so a node of either architecture resolves it. That is why the rule against\n",
                "# a `latest` base is suppressed on the next line.\n",
                "# hadolint ignore=DL3007\n",
                "FROM {registry}/et-ws-server:latest\n",
                "\n",
            ),
            registry = IMAGE_REGISTRY,
        );
    }
    format!(
        concat!(
            "# `hub` is a named build context, not a stage defined here.\n",
            "# The generated compose file wires it to its `{service}` service, so compose builds the hub image\n",
            "# first and hands the result over under this name. That is also why the version-tag rule is\n",
            "# suppressed on the next line: hadolint reads `hub` as an untagged image, but there is no tag to give a\n",
            "# build context, and the image it resolves to is pinned by the hub Dockerfile rather than here.\n",
            "# hadolint ignore=DL3006\n",
            "FROM hub\n",
            "\n",
        ),
        service = super::HUB_IMAGE_BUILD,
    )
}

/// Render the scenario image, which layers this cluster's modules onto the hub image.
fn render_dockerfile(cluster_name: &str, published: bool, sources: &ImageSources) -> String {
    let mut out = render_dockerfile_header(cluster_name, published);

    if !sources.mise_tools.is_empty() {
        out.push_str(&render_deps_stage(&sources.mise_tools));
    }
    if !sources.released.is_empty() {
        out.push_str(&render_released_stage(&sources.released));
    }

    // Each section above ends with its own blank line, so none of them start with one. Doing it the other way round
    // doubles the separator wherever two sections meet.
    out.push_str(&render_hub_base(published));
    out.push_str(concat!(
        "LABEL org.opencontainers.image.source=\"",
        et_org::repository_url!(),
        "\"\n",
        "\n",
    ));

    // `pkg/` unconditionally, never probed for on disk. Every module serves out of `pkg/`, whether that directory is
    // committed (the JS shims) or produced by a build (wasm-pack output, wheels). Probing for it makes this file depend
    // on what happens to be built in the working tree: CI checks out unbuilt Rust modules, found no `services/ws-
    // modules/har1/pkg`, and emitted the module root instead -- so the committed output and the CI regeneration
    // disagreed, and the drift check failed. A module that genuinely has no `pkg/` now fails loudly at image build
    // rather than silently generating a different Dockerfile per machine.
    for (repo_path, docker_path) in &sources.repo_paths {
        let _write_result = writeln!(out, "COPY --chown=10001:10001 {repo_path}/pkg {docker_path}/pkg");
    }
    // One copy of the whole staged tree, which mirrors `/app` path for path.
    if !sources.released.is_empty() {
        out.push_str("COPY --from=released --chown=10001:10001 /staged/app /app\n");
    }
    for (_, _, docker_path) in &sources.mise_tools {
        let _write_result = writeln!(
            out,
            "COPY --from=deps --chown=10001:10001 /staged{docker_path} {docker_path}"
        );
    }
    // Each from the named build context its `module_paths:` entry is handed to the build as, and whole rather than
    // its `pkg/`: whether the module serves from `pkg/` or its root is the scenario author's layout, and the hub finds
    // either once the directory is there. hadolint reads a context name as an undefined stage, as with `hub` above.
    for (context, context_path, docker_path) in &sources.scenario_dirs {
        let operands = format!("[{}, {}]", dockerfile_word(context_path), dockerfile_word(docker_path));
        let _write_result = writeln!(
            out,
            "# hadolint ignore=DL3022\nCOPY --from={context} --chown=10001:10001 {operands}"
        );
    }
    // The user the hub image already runs as, restated so the image says so itself rather than only through a base
    // a scanner cannot see into; every COPY above hands its files to the same id. An image with no COPY already ends
    // in the blank line that closes its LABEL.
    if !out.ends_with("\n\n") {
        out.push('\n');
    }
    out.push_str("USER 10001\n");

    out
}

/// The fixed head of the deps stage: the base image, the packages it installs, and a verified mise.
///
/// Held as a const rather than inlined so the rendering function below stays a short composer of named parts. Nothing
/// in this block varies with the cluster, so there is nothing here to parameterise.
const DEPS_BASE_STAGE: &str = concat!(
    "# Stage the packages that live outside the repository.\n",
    "# These are published packages the modules load at runtime, provisioned by mise rather than built\n",
    "# here, so this stage installs them from the repo's own pins and copies each one to the `/app` path\n",
    "# the hub serves it from.\n",
    "FROM docker.io/library/debian:bookworm-slim AS deps\n",
    "\n",
    "# The package list is named rather than inlined so a policy can hold it against the repo-root Dockerfile.\n",
    "# Every name here is drawn from that file's own COMMON_PACKAGES: the shell, the archive tools mise unpacks\n",
    "# downloads with, plus git and curl. Its compiler packages are deliberately absent -- nothing is compiled\n",
    "# in this stage -- so the two lists are checked as subset rather than equality.\n",
    "# Promoted to ENV because the quoted heredoc below is expanded by bash at run time, which an ARG alone\n",
    "# does not reach.\n",
    "ARG COMMON_PACKAGES=\"bash bzip2 ca-certificates curl git gzip tar unzip\"\n",
    "ENV COMMON_PACKAGES=${COMMON_PACKAGES}\n",
    "RUN bash <<'EOF'\n",
    "set -euo pipefail\n",
    "apt-get update\n",
    "# Word-splitting $COMMON_PACKAGES into separate arguments is the intent, so it is deliberately unquoted.\n",
    "apt-get install -y --no-install-recommends $COMMON_PACKAGES\n",
    "apt-get clean\n",
    "rm -rf /var/lib/apt/lists/* /var/cache/apt/archives/*.deb\n",
    "EOF\n",
    "\n",
    "# mise is fetched to a file and checksum-verified, never piped from mise.run into a shell.\n",
    "# Piping a remote script into an interpreter runs unreviewed code straight off the network, which the\n",
    "# repo's no-curl-pipe rule bans. The release's SHASUMS256.txt covers every asset, so `--ignore-missing`\n",
    "# checks exactly the archive downloaded here. Pinned rather than installed from mise's apt repository,\n",
    "# which carries only the newest version: a floating mise writes lockfile entries an older CI mise\n",
    "# then refuses to install.\n",
    "ARG MISE_VERSION=v2026.9.17\n",
    "ARG TARGETARCH\n",
    "ENV MISE_VERSION=${MISE_VERSION}\n",
    "ENV TARGETARCH=${TARGETARCH}\n",
    "RUN bash <<'EOF'\n",
    "set -euo pipefail\n",
    "case \"${TARGETARCH}\" in\n",
    "    amd64) arch=x64 ;;\n",
    "    arm64) arch=arm64 ;;\n",
    "    *) echo \"unsupported TARGETARCH ${TARGETARCH}\" >&2; exit 1 ;;\n",
    "esac\n",
    "asset=\"mise-${MISE_VERSION}-linux-${arch}.tar.gz\"\n",
    "base=\"https://github.com/jdx/mise/releases/download/${MISE_VERSION}\"\n",
    "cd /tmp\n",
    "curl -fsSL -o \"${asset}\" \"${base}/${asset}\"\n",
    "curl -fsSL -o SHASUMS256.txt \"${base}/SHASUMS256.txt\"\n",
    "sha256sum -c --ignore-missing SHASUMS256.txt\n",
    "tar -xzf \"${asset}\"\n",
    "install -m 0755 /tmp/mise/bin/mise /usr/local/bin/mise\n",
    "rm -rf /tmp/mise \"/tmp/${asset}\" /tmp/SHASUMS256.txt\n",
    "EOF\n",
    "\n",
    "ENV HOME=/root\n",
    "ENV PATH=\"/root/.local/share/mise/shims:${PATH}\"\n",
    "WORKDIR /workspace\n",
    "COPY .miserc.toml .miserc.toml\n",
    "COPY .mise/ .mise/\n",
    "RUN mise trust\n",
);

/// The per-tool copy step, driven entirely by the `STAGE_*` variables the loop emits ahead of it.
///
/// Locates the package rather than assuming where the backend put it. The npm backend nests it under one of several
/// layouts (`lib/node_modules/`, `node_modules/`, or an aube virtual store below a content-hashed directory) that vary
/// by platform, so this searches for the named directory instead. The named directory is tried FIRST and the install
/// root only as a fallback: the npm backend drops its own wrapper manifest (`"name": "mise-npm-install"`) at that root,
/// so a root-first probe stages the wrapper and the module is then served under the wrapper's name. The fallback is
/// what covers an archive-backed `http:` tool, which extracts flat so the root really is the package. `-L` and `cp -L`
/// resolve the symlinks the aube store is built from, so real files land in the image.
const DEPS_STAGE_TOOL_COPY: &str = concat!(
    "RUN bash <<'EOF'\n",
    "set -euo pipefail\n",
    "root=\"$(mise where \"${STAGE_TOOL}\")\"\n",
    "src=\"$(find -L \"${root}\" -type d -name \"${STAGE_PACKAGE}\" ",
    "-exec test -f '{}/package.json' ';' -print -quit)\"\n",
    "if [ -z \"${src}\" ] && [ -f \"${root}/package.json\" ]; then\n",
    "    src=\"${root}\"\n",
    "fi\n",
    "if [ -z \"${src}\" ]; then\n",
    "    echo \"no ${STAGE_PACKAGE} package dir under the ${STAGE_TOOL} install at ${root}\" >&2\n",
    "    exit 1\n",
    "fi\n",
    "mkdir -p \"$(dirname \"${STAGE_DEST}\")\"\n",
    "cp -rL \"${src}\" \"${STAGE_DEST}\"\n",
    "EOF\n",
    "\n",
);

/// Image the released-modules stage runs `npm` from, on the Node major the repository's own `node` tool is pinned to.
const NODE_IMAGE: &str = "docker.io/library/node:22-bookworm-slim";

/// Build secret carrying the token GitHub Packages reads are made with, the id the repo-root Dockerfile uses too.
pub(crate) const REGISTRY_TOKEN_SECRET: &str = "gh_token";

/// Environment variable the registry token is read from on the host and exposed as inside the fetching step.
pub(crate) const REGISTRY_TOKEN_ENV: &str = "GITHUB_TOKEN";

/// Render the stage that fetches this repository's modules from the registry a published scenario consumes them from.
///
/// `npm pack` rather than a download by URL, because it checks each tarball against the integrity the registry records
/// for it before writing it. The tarball holds the module's `pkg/` exactly as it was published, under npm's `package/`
/// prefix, which the extraction strips. The token reaches the step as a secret mount, never as a build argument or a
/// file in a layer, and the npm config only names it: npm expands `${GITHUB_TOKEN}` when it reads the file.
fn render_released_stage(released: &[(String, String)]) -> String {
    // One `printf` per line, appending, so no line of the step outgrows the file's width.
    let npmrc = super::mise::npmrc_contents()
        .lines()
        .fold(String::default(), |mut steps, line| {
            let _write_result = writeln!(steps, "printf '%s\\n' '{line}' >> /released.npmrc");
            steps
        });
    let mut stage = format!(
        concat!(
            "# Fetch the released modules this scenario serves.\n",
            "# GitHub Packages refuses an anonymous read even of a public package, so the build takes a `gh_token`\n",
            "# secret, which is only ever mounted into the step that needs it.\n",
            "FROM {image} AS released\n",
            "\n",
            "ENV NPM_CONFIG_USERCONFIG=/released.npmrc\n",
            "RUN bash <<'EOF'\n",
            "set -euo pipefail\n",
            "{npmrc}",
            "EOF\n",
            "\n",
        ),
        image = NODE_IMAGE,
        npmrc = npmrc,
    );
    for (package, docker_path) in released {
        let _write_result = writeln!(stage, "ENV STAGE_PACKAGE=\"{package}\"");
        let _write_result = writeln!(stage, "ENV STAGE_DEST=\"/staged{docker_path}/pkg\"");
        let _write_result = writeln!(
            stage,
            "RUN --mount=type=secret,id={REGISTRY_TOKEN_SECRET},env={REGISTRY_TOKEN_ENV} bash <<'EOF'"
        );
        stage.push_str(concat!(
            "set -euo pipefail\n",
            "cd /tmp\n",
            "tarball=\"$(npm pack --silent \"${STAGE_PACKAGE}@latest\")\"\n",
            "mkdir -p \"${STAGE_DEST}\"\n",
            "tar -xzf \"${tarball}\" -C \"${STAGE_DEST}\" --strip-components=1\n",
            "rm \"${tarball}\"\n",
            "EOF\n",
            "\n",
        ));
    }
    stage
}

/// Render the stage that installs the mise-staged packages the cluster's modules depend on.
///
/// The repo's own `.mise/` configs and lockfiles are the version source, so nothing here pins a version that could
/// drift from what a workstation resolves; only the tool ids come from the module registry.
fn render_deps_stage(mise_tools: &[(String, String, String)]) -> String {
    let tools = mise_tools
        .iter()
        .map(|(tool, _, _)| tool.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    let mut stage = String::default();
    stage.push_str(DEPS_BASE_STAGE);
    let _write_result = writeln!(stage, "ENV MISE_ENV={DEPS_MISE_ENV}");
    if mise_tools.iter().any(|(tool, _, _)| tool.starts_with("npm:")) {
        // mise's npm backend shells out to npm, and refuses an `npm:` tool whose configured `node` dependency is
        // not installed yet ("requires configured install dependency 'node@22', but its selected version is not
        // installed"). Its version comes from the repo's own [tools] pin, like every other tool staged here. The
        // blank line after `EOF` is load-bearing, not formatting. hadolint's parser reads the next instruction as a
        // continuation of the heredoc without it and fails with `unexpected 'E' expecting a new line followed by the
        // next instruction`.
        stage.push_str(concat!(
            "RUN bash <<'EOF'\n",
            "set -euo pipefail\n",
            "mise install node\n",
            "EOF\n",
            "\n",
        ));
    }
    let _write_result = writeln!(stage, "ENV SCENARIO_TOOLS=\"{tools}\"");
    stage.push_str(concat!(
        "RUN bash <<'EOF'\n",
        "set -euo pipefail\n",
        "# Word-splitting SCENARIO_TOOLS is intentional; it is a space-separated tool list.\n",
        "# shellcheck disable=SC2086\n",
        "mise install ${SCENARIO_TOOLS}\n",
        "EOF\n",
        "\n",
    ));
    for (tool, package, docker_path) in mise_tools {
        let _write_result = writeln!(stage, "ENV STAGE_TOOL=\"{tool}\"");
        let _write_result = writeln!(stage, "ENV STAGE_PACKAGE=\"{package}\"");
        let _write_result = writeln!(stage, "ENV STAGE_DEST=\"/staged{docker_path}\"");
        stage.push_str(DEPS_STAGE_TOOL_COPY);
    }
    stage
}

/// Render the ignore file `BuildKit` applies to this Dockerfile in place of the repository root one.
///
/// It is the root file verbatim plus a re-include block, rather than a hand-picked subset, so a rule added to
/// `.gitignore` reaches this build too. Docker's matcher is last-match-wins and, unlike git's, still descends into an
/// excluded directory to honour a later negation, so the trailing block is enough to bring back the built artifacts
/// while every other exclusion keeps applying.
fn render_dockerignore(
    project_root: &Path,
    cluster_name: &str,
    repo_paths: &[(String, String)],
) -> Result<String, CliError> {
    let root = fs::read_to_string(project_root.join(".dockerignore"))?;
    let mut out = String::default();
    let _write_result = writeln!(
        out,
        concat!(
            "# AUTO-GENERATED by `et-cli` for the `{name}` scenario -- do not edit.\n",
            "# Regenerate with `mise run regen-verification`.\n",
            "#\n",
            "# BuildKit reads this file instead of the repository root `.dockerignore` when building the\n",
            "# Dockerfile beside it, so the root file keeps governing the CI images built from `./Dockerfile`\n",
            "# and only this build sees the re-includes at the end.\n",
        ),
        name = cluster_name,
    );
    out.push_str(&root);
    out.push_str(concat!(
        "\n",
        "# Re-admit the built module artifacts this scenario serves.\n",
        "# The rules above drop every `*.wasm` and `*.onnx`, which is what keeps the CI images small; these\n",
        "# negations put back the `pkg/` directories this image copies, and nothing else.\n",
    ));
    // A BTreeSet both de-duplicates paths shared by two modules and keeps the emitted order stable.
    let negations: BTreeSet<String> = repo_paths
        .iter()
        .map(|(repo_path, _)| format!("!{repo_path}/pkg/**"))
        .collect();
    for negation in negations {
        out.push_str(&negation);
        out.push('\n');
    }

    Ok(out)
}
