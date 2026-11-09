use std::collections::BTreeSet;
use std::path::Path;

use fs_err as fs;
use serde::Serialize;

use crate::error::CliError;
use crate::input::{ArtifactSource, ClusterInput};
use crate::templates::render;
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

/// Image the released-modules stage runs `npm` from, on the Node major the repository's own `node` tool is pinned to.
const NODE_IMAGE: &str = "docker.io/library/node:22-bookworm-slim";

/// Build secret carrying the token GitHub Packages reads are made with, the id the repo-root Dockerfile uses too.
pub(crate) const REGISTRY_TOKEN_SECRET: &str = "gh_token";

/// Environment variable the registry token is read from on the host and exposed as inside the fetching step.
pub(crate) const REGISTRY_TOKEN_ENV: &str = "GITHUB_TOKEN";

/// Where the scenario image's modules come from, split by what each source needs from the build.
///
/// Each list is sorted out of resolution order, which moves a module whenever an unrelated one gains a dependency, so
/// every list the image is rendered from stays put in a committed file: `SCENARIO_TOOLS`, the staging steps, the COPYs.
#[derive(Default)]
struct ImageSources {
    /// This repository's modules, copied from the checkout.
    repo_paths: Vec<RepoModule>,
    /// This repository's modules, fetched as the releases a published scenario consumes.
    released: Vec<ReleasedModule>,
    /// Packages a mise tool provisions, staged from the repository's own pins.
    mise_tools: Vec<StagedTool>,
    /// The scenario's own `module_paths:` directories.
    scenario_dirs: Vec<ScenarioDir>,
}

/// One of this repository's modules, copied from the checkout.
#[derive(Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
struct RepoModule {
    repo_path: String,
    docker_path: String,
}

/// One of this repository's modules, fetched as the release a published scenario consumes.
#[derive(Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
struct ReleasedModule {
    package: String,
    docker_path: String,
}

/// A package a mise tool provisions, located by `package` below the install of `tool`.
#[derive(Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
struct StagedTool {
    tool: String,
    package: String,
    docker_path: String,
}

/// One of the scenario's own `module_paths:` directories, at `context_path` within the build context `context`.
#[derive(Debug, Eq, Ord, PartialEq, PartialOrd)]
struct ScenarioDir {
    context: String,
    context_path: String,
    docker_path: String,
}

/// A [`ScenarioDir`] as its `COPY` names it, each path already one operand of the JSON-array form.
#[derive(Serialize)]
struct ScenarioCopy<'sources> {
    context: &'sources str,
    source: String,
    destination: String,
}

/// What the scenario Dockerfile is rendered from.
#[derive(Serialize)]
struct DockerfileContext<'sources> {
    name: &'sources str,
    published: bool,
    /// Whether a staged tool is an `npm:` one, which needs `node` installed first.
    installs_node: bool,
    repo_paths: &'sources [RepoModule],
    released: &'sources [ReleasedModule],
    mise_tools: &'sources [StagedTool],
    scenario_dirs: Vec<ScenarioCopy<'sources>>,
    /// The npm config the released stage fetches with, as a published deployment carries it.
    npmrc: String,
    deps_mise_env: &'static str,
    hub_dockerfile: &'static str,
    hub_image_build: &'static str,
    node_image: &'static str,
    registry: &'static str,
    repository_url: &'static str,
    token_env: &'static str,
    token_secret: &'static str,
}

/// What the scenario Dockerfile's ignore file is rendered from.
#[derive(Serialize)]
struct DockerignoreContext<'sources> {
    name: &'sources str,
    /// Whether the image builds from the repository, rather than from the deployment directory with nothing sent.
    repository: bool,
    /// The repository's own ignore file, which an image built from the repository starts from.
    root_dockerignore: String,
    /// The `pkg/` tree of each module copied from the repository, as a pattern; sorted and without repeats.
    built_artifacts: BTreeSet<String>,
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
            ModuleSource::Repo(_) if published => sources.released.push(ReleasedModule {
                docker_path,
                package: entry.package_name.unwrap_or_default(),
            }),
            ModuleSource::Repo(repo_path) => sources.repo_paths.push(RepoModule { repo_path, docker_path }),
            ModuleSource::MiseTool { tool, package } => sources.mise_tools.push(StagedTool {
                tool,
                package,
                docker_path,
            }),
            ModuleSource::Scenario {
                context, context_path, ..
            } => sources.scenario_dirs.push(ScenarioDir {
                context,
                context_path,
                docker_path,
            }),
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

pub fn generate_scenario_image(cluster: &ClusterInput, output_dir: &Path) -> Result<(), CliError> {
    let sources = image_sources(cluster)?;
    let published = matches!(cluster.artifact_source, ArtifactSource::Published);
    let dockerfile = render_dockerfile(&cluster.cluster_name, published, &sources)?;
    fs::write(output_dir.join("Dockerfile"), dockerfile)?;
    let dockerignore = render_dockerignore(
        &cluster.cluster_name,
        needs_of(cluster, &sources).repository,
        &sources.repo_paths,
    )?;
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

/// Render the scenario image, which layers this cluster's modules onto the hub image.
fn render_dockerfile(cluster_name: &str, published: bool, sources: &ImageSources) -> Result<String, CliError> {
    let scenario_dirs = sources
        .scenario_dirs
        .iter()
        .map(|dir| ScenarioCopy {
            context: &dir.context,
            destination: dockerfile_word(&dir.docker_path),
            source: dockerfile_word(&dir.context_path),
        })
        .collect();
    render(
        "scenario-image.j2",
        &DockerfileContext {
            deps_mise_env: DEPS_MISE_ENV,
            hub_dockerfile: HUB_DOCKERFILE,
            hub_image_build: super::HUB_IMAGE_BUILD,
            installs_node: sources.mise_tools.iter().any(|staged| staged.tool.starts_with("npm:")),
            mise_tools: &sources.mise_tools,
            name: cluster_name,
            node_image: NODE_IMAGE,
            npmrc: super::mise::npmrc_contents()?,
            published,
            registry: IMAGE_REGISTRY,
            released: &sources.released,
            repo_paths: &sources.repo_paths,
            repository_url: et_org::repository_url!(),
            scenario_dirs,
            token_env: REGISTRY_TOKEN_ENV,
            token_secret: REGISTRY_TOKEN_SECRET,
        },
    )
}

/// Render the ignore file the scenario Dockerfile is built with, from the repository's own when it builds from there.
fn render_dockerignore(cluster_name: &str, repository: bool, repo_paths: &[RepoModule]) -> Result<String, CliError> {
    let root_dockerignore = if repository {
        fs::read_to_string(edge_toolkit::config::get_project_root().join(".dockerignore"))?
    } else {
        String::default()
    };
    render(
        "scenario-image.dockerignore.j2",
        &DockerignoreContext {
            built_artifacts: repo_paths
                .iter()
                .map(|module| format!("{}/pkg/**", module.repo_path))
                .collect(),
            name: cluster_name,
            repository,
            root_dockerignore,
        },
    )
}
