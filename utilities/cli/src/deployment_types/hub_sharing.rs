//! What several hubs share, which every deployment format runs on the server once a cluster has more than one hub.
//!
//! NATS carries the agent directory, the mailbox and delivery between hubs, and rustfs is the S3 object store every
//! hub's agent storage points at. Both run from their upstream images in every format, mise included: rustfs's pinned
//! release cannot initialise its storage on Windows, and a container is the one way it runs the same everywhere.

use edge_toolkit::ports::Services;

use super::mise::COLLECTOR_TASK;

/// The task, service and `Deployment` that runs NATS.
pub(crate) const BACKPLANE_TASK: &str = "nats";

/// The task, service and `Deployment` that runs rustfs.
pub(crate) const SHARED_STORAGE_TASK: &str = "rustfs";

/// The NATS image, at the release the repository's own `nats-server` tool is pinned to.
pub(crate) const BACKPLANE_IMAGE: &str = "nats:2.15.1-alpine";

/// The rustfs image, at the release the repository's own `rustfs` tool is pinned to.
pub(crate) const SHARED_STORAGE_IMAGE: &str = "rustfs/rustfs:1.0.0-beta.12";

/// Where each of the two keeps its data inside its container, which is what its volume is mounted over.
pub(crate) const DATA_DIR: &str = "/data";

/// The bucket every hub's agent storage is kept in, keyed `<agent>/<file>` as the hub's local store is.
const BUCKET: &str = "et-storage";

/// The account the hubs present to rustfs: not a secret, so stated wherever it is needed, like the collector's.
pub(crate) const STORAGE_ACCOUNT: &str = "et-storage";

/// The region the S3 client signs requests for; rustfs accepts any, and the client requires one.
const REGION: &str = "us-east-1";

/// The variables the scenario's env file carries the storage secret in: the hubs' S3 client's, then rustfs's.
pub(crate) const STORAGE_SECRET_ENVS: [&str; 2] = ["AWS_SECRET_ACCESS_KEY", "RUSTFS_SECRET_KEY"];

/// The path NATS answers healthy on once `JetStream` is up, which is what a hub needs from it.
pub(crate) const BACKPLANE_HEALTH_PATH: &str = "/healthz?js-enabled-only=true";

/// The path rustfs answers healthy on.
pub(crate) const SHARED_STORAGE_HEALTH_PATH: &str = "/health";

/// Where the NATS image's own `CMD` reads its configuration file from.
pub(crate) const BACKPLANE_CONFIG_DIR: &str = "/etc/nats";

/// The NATS configuration file's name, under [`BACKPLANE_CONFIG_DIR`].
pub(crate) const BACKPLANE_CONFIG_FILE: &str = "nats-server.conf";

/// Each container the server runs as a task, with the port and path its readiness check asks.
///
/// The collector always, and what the hubs share when `shared`: the tasks the server's hub waits on.
pub(crate) fn server_containers(shared: bool) -> Vec<(&'static str, u16, &'static str)> {
    let mut containers = vec![(COLLECTOR_TASK, Services::OtlpCollector.port(), "/healthz")];
    if shared {
        containers.extend([
            (BACKPLANE_TASK, Services::BackplaneMonitor.port(), BACKPLANE_HEALTH_PATH),
            (
                SHARED_STORAGE_TASK,
                Services::SharedStorage.port(),
                SHARED_STORAGE_HEALTH_PATH,
            ),
        ]);
    }
    containers
}

/// The NATS server's arguments, for the formats that start it from the command line.
///
/// `JetStream`, which the backplane needs, kept in the data volume, and the monitoring port its health check is served
/// on. Every one is a flag, so the image's entrypoint runs them as `nats-server` arguments rather than its default
/// config file. They say what [`backplane_config`] does.
pub(crate) fn backplane_args() -> Vec<String> {
    vec![
        "--jetstream".to_string(),
        "--store_dir".to_string(),
        DATA_DIR.to_string(),
        "--http_port".to_string(),
        Services::BackplaneMonitor.port().to_string(),
    ]
}

/// The NATS server's configuration file, for k3s, which runs the image's own `CMD` over it rather than arguments.
///
/// It says what [`backplane_args`] does: the client port, the monitoring port and `JetStream` kept in the data volume.
pub(crate) fn backplane_config() -> String {
    format!(
        "port: {client}\nmonitor_port: {monitor}\njetstream {{\n  store_dir: {DATA_DIR}\n}}\n",
        client = Services::Backplane.port(),
        monitor = Services::BackplaneMonitor.port(),
    )
}

/// The shell command rustfs is started with: create the bucket, then run the image's own entrypoint.
///
/// rustfs creates no bucket on a write to one it does not have, but adopts a directory already in its volume as a
/// bucket. Making it here, before the server starts, is what lets the hubs write without anything else running first.
pub(crate) fn shared_storage_start() -> String {
    format!("mkdir -p {DATA_DIR}/{BUCKET} && exec /entrypoint.sh rustfs")
}

/// rustfs's own settings, besides the secret the env file carries.
pub(crate) fn shared_storage_env() -> Vec<(&'static str, String)> {
    vec![
        ("RUSTFS_ACCESS_KEY", STORAGE_ACCOUNT.to_string()),
        ("RUSTFS_VOLUMES", DATA_DIR.to_string()),
    ]
}

/// The environment that joins a hub to the others: NATS at `backplane_host`, rustfs at `storage_host`.
///
/// The storage secret is not among them; it arrives from the env file, beside the collector credential.
/// `AWS_ALLOW_HTTP` because rustfs is reached in plaintext, as everything between these machines is.
pub(crate) fn shared_hub_env(backplane_host: &str, storage_host: &str) -> Vec<(&'static str, String)> {
    vec![
        ("AWS_ACCESS_KEY_ID", STORAGE_ACCOUNT.to_string()),
        ("AWS_ALLOW_HTTP", "true".to_string()),
        ("AWS_DEFAULT_REGION", REGION.to_string()),
        (
            "AWS_ENDPOINT",
            format!("http://{storage_host}:{}", Services::SharedStorage.port()),
        ),
        ("STORAGE_URL", format!("s3://{BUCKET}")),
        (
            "WS_BACKPLANE",
            format!("nats://{backplane_host}:{}", Services::Backplane.port()),
        ),
    ]
}
