# math1-multi

This directory contains generated deployment configs for the `math1-multi` scenario.
Files: `mise.toml`, `compose.yaml`, `k3s.yaml`, `dekit.yaml`.

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

Fetch the binaries and module packages the `mise.toml` tasks name before the first run.
`GITHUB_TOKEN` has to be set: GitHub Packages rejects an unauthenticated read even for a public
package. The registry configuration is exported rather than relied on from `mise.toml`, because
mise does not apply its own `[env]` to the resolution this command performs:

```bash
NPM_CONFIG_USERCONFIG="$PWD/npmrc" mise install
```

## Run With Mise

From this directory, start the scenario with:

```bash
mise run generated-scenario
```

That task starts both OpenObserve and `ws-server` for this scenario.

### Open The OpenObserve UI

From this directory, open the OpenObserve UI with:

```bash
mise run open-o2
```

### Run Across Machines With Mise

Copy this directory to each machine -- for a scenario built from the repository, into a checkout
of it at the same path -- and start that machine's share of the scenario there.
`server` runs OpenObserve and `ws-server`, and the runners on every other machine reach
the hub at `127.0.0.1`.

On `server`:

```bash
mise run machine-server
```

On `edge`:

```bash
mise run machine-edge
```

## Run With Docker Compose

From this directory, start the scenario with:

```bash
docker compose up --build
```

The compose stack starts OpenObserve and builds one image: the `Dockerfile` in this directory,
which stages this scenario's modules onto the released hub image it names as a build context.
`ws-server` runs with host networking so it advertises the same LAN IP as the `mise` deployment.

### Open The UIs

OpenObserve is available at <http://localhost:5080/>.
`ws-server` is available at <http://localhost:8080/> and <https://localhost:8443/>.

Stop the scenario with:

```bash
docker compose down
```

### Run Across Machines With Docker Compose

Copy this directory to each machine -- for a scenario built from the repository, into a checkout
of it at the same path -- and start only that machine's profile there.
`server` runs OpenObserve and `ws-server`, and the runners on every other machine reach
the hub at `127.0.0.1`.

On `server`:

```bash
docker compose --profile server up --build
```

On `edge`:

```bash
docker compose --profile edge up --build
```

## Run With k3s

The manifests reference images by name and never build them, so this scenario's own image has
to reach the node first. It layers its modules onto the module-less hub image and takes that
hub as a _named build context_ rather than building it, so a plain `docker build` of the
scenario Dockerfile fails on `FROM hub` -- pointing the context at the published hub is what
supplies it. From this directory,
with `GITHUB_TOKEN` set to a token that can read the GitHub Packages the build fetches this
scenario's released modules from:

```bash
scenario=math1-multi
hub=ghcr.io/edge-toolkit/core/et-ws-server:latest
image="et-ws-server-$scenario:latest"
docker build --build-context "hub=docker-image://$hub" --secret id=gh_token,env=GITHUB_TOKEN -t "$image" .
docker save "$image" | sudo k3s ctr images import -
```

The cluster pulls this scenario's runner images, so nothing has to be built or imported for
them:

- `ghcr.io/edge-toolkit/core/et-ws-web-runner:latest`

### Load The Credential

The credential reaches the pods as a `Secret` created from `secrets.env`, rather than written into
`k3s.yaml` where it would be committed alongside the manifests. From this directory:

```bash
ns=et-math1-multi
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

### Run Across Machines With k3s

The machines have to form one k3s cluster first, each labelled with the machine name the manifests
pin pods to. With `k3s` installed on every machine, start the server on `server`. It stays in
the foreground, so read the token the others join with from a second shell once it is up:

```bash
sudo k3s server --node-label edge-toolkit/machine=server
```

```bash
sudo cat /var/lib/rancher/k3s/server/node-token
```

Then join each other machine, with that token in `token`:

```bash
server=127.0.0.1
sudo k3s agent --server "https://${server}:6443" --token "$token" --node-label edge-toolkit/machine=edge
```

Each image goes to the machines that run it rather than to one node, so the import steps above
become these, with each machine reachable over `ssh` by its address:

```bash
image="et-ws-server-math1-multi:latest"
docker save "$image" | ssh 127.0.0.1 sudo k3s ctr images import -
```

## Run With dekit

From this directory, start the scenario and watch each process in its own pane with:

```bash
dekit up
dekit attach
```

Each task runs one task of `mise.toml`, so these are the processes `mise run generated-scenario`
starts, but in order: `ws-server` waits for OpenObserve to report healthy, and every runner
waits for `ws-server`. The tasks keep running after the terminal closes; stop them with:

```bash
dekit down
```

Open the OpenObserve UI, once it is up, with:

```bash
dekit start open-o2
```

### Run Across Machines With dekit

Copy this directory to each machine and start that machine's share there. Every task is tagged
with its machine and none starts on `dekit up` alone, so `dekit up` on any machine starts nothing
until its tag is started.
`server` runs OpenObserve and `ws-server`, and the runners on every other machine reach
the hub at `127.0.0.1`.

On `server`:

```bash
dekit up
dekit start +server
```

On `edge`:

```bash
dekit up
dekit start +edge
```
