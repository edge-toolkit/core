# math1-cron

This directory contains generated deployment configs for the `math1-cron` scenario.
Files: `mise.toml`, `process-compose.yaml`, `compose.yaml`, `k3s.yaml`.

The scenario exposes these workflow modules: math1, math1-sender.

This scenario is rendered against released artifacts rather than builds of this repository:
the images it names are published, and the binaries it runs are the released ones. Only the
image carrying its own module set is still built locally, since no release can hold a module
set particular to one deployment.

`secrets.env` holds the scenario's derived OpenObserve and OTLP credentials. It is derived from the
scenario input, so regenerating this deployment rewrites it; if it is missing, regenerate before
starting the stack. A deployment generated outside the repository is written with a `.gitignore`
covering it, so its credential is not committed by whatever repository it lands in.

## Install

Lock and fetch the binaries and module packages the `mise.toml` tasks name before the first run.
`GITHUB_TOKEN` has to be set: GitHub Packages rejects an unauthenticated read even for a public
package. The registry configuration is exported rather than relied on from `mise.toml`, because
mise does not apply its own `[env]` to the resolution these commands perform:

```bash
export NPM_CONFIG_USERCONFIG="$PWD/npmrc"
mise lock
mise install --locked
```

`--locked` installs exactly what the lockfile records, and fails rather than resolving anew
when it is missing an entry.

Commit `mise.lock` alongside the deployment, together with the `.mise/locks/` directory when
mise writes one -- it holds the dependency graph of each npm tool, which `mise.lock` refers to.
The two record the version and checksum every `latest` in `mise.toml` resolved to, for each of
mise's common platforms, so every later install -- on any machine -- fetches the same binaries
rather than whatever is newest that day. Move to newer releases deliberately, with
`mise lock --bump`, and commit the result.

## Run With process-compose

From this directory, start the scenario and watch each process with:

```bash
process-compose up --no-server -f process-compose.yaml
```

Each process runs one task of `mise.toml`, in order: `ws-server` waits for OpenObserve to report
healthy, and every runner waits for `ws-server`. A scheduled agent's runner is started on each
tick of its schedule and runs its module once; a tick that finds the previous run still going is
skipped. `--no-server` keeps process-compose's own API off port 8080, where `ws-server` listens.
Quitting the TUI stops every process. Open the OpenObserve UI, once it is up, by starting the
`open-o2` process from the TUI.

## Run With Docker Compose

From this directory, start the scenario with:

```bash
docker compose up --build
```

The compose stack starts OpenObserve and builds one image: the `Dockerfile` in this directory,
which stages this scenario's modules onto the released hub image it names.
`ws-server` runs with host networking so it advertises the same LAN IP as the `mise` deployment.

### Open The UIs

OpenObserve is available at <http://localhost:5080/>.
`ws-server` is available at <http://localhost:8080/> and <https://localhost:8443/>.

Stop the scenario with:

```bash
docker compose down
```

### Scheduled Agents With Docker Compose

`docker compose up` runs each scheduled agent's runner once, as it does every service. The
`scheduler` service, Ofelia, then starts that same container again on each tick of its schedule,
skipping a tick while the previous run is still going. It mounts the Docker socket to do so, which
gives it the Docker daemon's privileges.

## Run With k3s

The manifests reference images by name and never build them, so this scenario's own image has
to reach the node first. It layers its modules onto the released hub image, which its Dockerfile
names and the build pulls. From this directory,
with `GITHUB_TOKEN` set to a token that can read the GitHub Packages the build fetches this
scenario's released modules from:

```bash
scenario=math1-cron
image="et-ws-server-$scenario:latest"
docker build --secret id=gh_token,env=GITHUB_TOKEN -t "$image" .
docker save "$image" | sudo k3s ctr images import -
```

The cluster pulls this scenario's runner images, so nothing has to be built or imported for
them:

- `ghcr.io/edge-toolkit/core/et-ws-web-runner:latest`

### Load The Credential

The credential reaches the pods as a `Secret` created from `secrets.env`, rather than written into
`k3s.yaml` where it would be committed alongside the manifests. From this directory:

```bash
ns=et-math1-cron
kubectl create namespace "$ns" --save-config
kubectl create secret generic "$ns-secrets" --from-env-file=secrets.env -n "$ns"
```

### Apply

```bash
kubectl apply -f k3s.yaml
```

The runners exit and restart until the hub reports ready, so `CrashLoopBackOff` while the hub
starts is expected here rather than a fault. Watch it settle with:

```bash
kubectl get pods -n "$ns" --watch
```
