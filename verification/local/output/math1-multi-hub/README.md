# math1-multi-hub

This directory contains generated deployment configs for the `math1-multi-hub` scenario.
Files: `mise.toml`, `dekit.yaml`, `process-compose.yaml`, `compose.yaml`, `k3s.yaml`.

The scenario exposes these workflow modules: math1, math1-sender.

`secrets.env` holds the scenario's derived OpenObserve, OTLP and object store credentials. It is derived from the
scenario input, so regenerating this deployment rewrites it; if it is missing, regenerate before
starting the stack. A deployment generated outside the repository is written with a `.gitignore`
covering it, so its credential is not committed by whatever repository it lands in.

## Lock The Tools

Before the first run, lock the tools `mise.toml` names:

```bash
mise lock
```

Commit `mise.lock` alongside the deployment, together with the `.mise/locks/` directory when
mise writes one -- it holds the dependency graph of each npm tool, which `mise.lock` refers to.
The two record the version and checksum every `latest` in `mise.toml` resolved to, for each of
mise's common platforms, so every later install -- on any machine -- fetches the same binaries
rather than whatever is newest that day. Move to newer releases deliberately, with
`mise lock --bump`, and commit the result.

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
`server` runs OpenObserve, `ws-server`, and the NATS server and object store the hubs share,
which the other machines' hubs reach at `127.0.0.1`. Each runner joins the hub on its own
machine, or the one on `server` when its machine runs none.

On `server`:

```bash
mise run machine-server
```

On `edge`:

```bash
mise run machine-edge
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
`server` runs OpenObserve, `ws-server`, and the NATS server and object store the hubs share,
which the other machines' hubs reach at `127.0.0.1`. Each runner joins the hub on its own
machine, or the one on `server` when its machine runs none.

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

## Run With process-compose

From this directory, install the process-compose `mise.toml` declares, then start the scenario
and watch each process with:

```bash
mise install
process-compose up --no-server -f process-compose.yaml
```

Each process runs one task of `mise.toml`, in order: `ws-server` waits for OpenObserve to report
healthy, and every runner waits for `ws-server`. A scheduled agent's runner is started on each
tick of its schedule and runs its module once; a tick that finds the previous run still going is
skipped. `--no-server` keeps process-compose's own API off port 8080, where `ws-server` listens.
Quitting the TUI stops every process. Open the OpenObserve UI, once it is up, by starting the
`open-o2` process from the TUI.

### Run Across Machines With process-compose

Copy this directory to each machine -- for a scenario built from the repository, into a checkout
of it at the same path -- and start only that machine's namespace there.
`server` runs OpenObserve, `ws-server`, and the NATS server and object store the hubs share,
which the other machines' hubs reach at `127.0.0.1`. Each runner joins the hub on its own
machine, or the one on `server` when its machine runs none.

On `server`:

```bash
process-compose up --no-server -f process-compose.yaml -n server
```

On `edge`:

```bash
process-compose up --no-server -f process-compose.yaml -n edge
```

## Run With Docker Compose

From this directory, start the scenario with:

```bash
docker compose up --build
```

The compose stack starts OpenObserve and builds `ws-server` in two layers: the module-less hub
image from the repository's `services/ws-server/Dockerfile`, then the `Dockerfile` in this
directory, which stages this scenario's modules onto it.
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
`server` runs OpenObserve, `ws-server`, and the NATS server and object store the hubs share,
which the other machines' hubs reach at `127.0.0.1`. Each runner joins the hub on its own
machine, or the one on `server` when its machine runs none.

On `server`:

```bash
docker compose --profile server up --build
```

On `edge`:

```bash
docker compose --profile edge up --build
```

## Run With k3s

The manifests reference images by name and never build them, so build and import each one first.
This scenario's image layers its modules onto the module-less hub image and takes that hub as a
_named build context_ rather than building it, so the hub has to exist first -- a plain
`docker build` of the scenario Dockerfile fails on `FROM hub`. From the repository root:

```bash
scenario=math1-multi-hub
hub=et-ws-server-hub:latest
image="et-ws-server-$scenario:latest"
dockerfile="verification/local/output/$scenario/Dockerfile"
docker build -t "$hub" -f services/ws-server/Dockerfile .
docker build --build-context "hub=docker-image://$hub" -t "$image" -f "$dockerfile" .
docker save "$image" | sudo k3s ctr images import -
```

Then this scenario's runner images, which need no build context of their own:

```bash
docker build -t et-ws-web-runner:latest -f services/ws-web-runner/Dockerfile .
docker save et-ws-web-runner:latest | sudo k3s ctr images import -
```

### Load The Credential

The credential reaches the pods as a `Secret` created from `secrets.env`, rather than written into
`k3s.yaml` where it would be committed alongside the manifests. From this directory:

```bash
ns=et-math1-multi-hub
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
image="et-ws-server-math1-multi-hub:latest"
docker save "$image" | ssh 127.0.0.1 sudo k3s ctr images import -
docker save "$image" | ssh 127.0.0.1 sudo k3s ctr images import -
docker save et-ws-web-runner:latest | ssh 127.0.0.1 sudo k3s ctr images import -
docker save et-ws-web-runner:latest | ssh 127.0.0.1 sudo k3s ctr images import -
```
