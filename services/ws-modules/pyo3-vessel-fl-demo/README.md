# pyo3-vessel-fl-demo

A demo-scale, three-client federated vessel classifier for `et-ws-pyo3-runner`.
Each client trains one small multimodal PyTorch model from its local prepared
log-mel features and paired camera images. The coordinator performs
sample-count-weighted FedAvg through Edge Toolkit storage and WebSocket pointer
messages. Raw samples never cross the hub.

The companion `et-ws-pyo3-mlflow` module observes generic tracking events and
logs them to an ordinary MLflow HTTP server. Tracking is fire-and-forget: the FL
workflow does not depend on the bridge being present.

## Dataset

The default dataset root is:

```text
~/Datasets/croatia-postprocessed/croatia_1g_logmel_images
```

Override it without changing the checked-in deployment:

```sh
export VESSEL_FL_DATA_ROOT=/absolute/path/to/croatia_1g_logmel_images
```

The module consumes the existing `clients/vessel_{1,2,3}` manifests,
`feature_manifest.csv`, `class_summary.csv`, `features/log_mel` tensors, and
`images/camera` images.

## Run

From the repository root, use the prototype deployment and install the demo's
pinned Python dependencies. `setup-python` places them in the ignored
module-local `.python-packages/` directory for the same CPython that the Pyo3
runner embeds; it does not alter the system Python:

```sh
mise -C verification/local/output/vessel-fl-demo -E prototype run setup-python
mise -C verification/local/output/vessel-fl-demo -E prototype run check-demo
mise -C verification/local/output/vessel-fl-demo -E prototype run ws-server
# In another terminal:
mise -C verification/local/output/vessel-fl-demo -E prototype run demo
```

Then open MLflow either with the command-line helper:

```sh
mise -C verification/local/output/vessel-fl-demo -E prototype run open-mlflow
```

or select **Vessel FL — open MLflow dashboard** in the Edge Toolkit workflow
module list and press **Run**. It opens the loopback-only URL
`http://127.0.0.1:5000`. Set `window.__VESSEL_FL_MLFLOW_URL` before loading the
frontend if MLflow is hosted at a different URL.

Open `http://127.0.0.1:8080` in four browser instances. Select **Vessel 1 FL
client**, **Vessel 2 FL client**, and **Vessel 3 FL client** in three instances
and press **Run** to attach each view to its native client. In the fourth,
select **Vessel FL coordinator** and press **Run** to start training and display
aggregation, evaluation, artifact, and MLflow activity. Training and dataset
access remain in the native Pyo3 agents.

For a headless or CI run that has no browser, opt into automatic start:

```sh
VESSEL_FL_AUTO_START=true mise -C verification/local/output/vessel-fl-demo -E prototype run demo
```

Important optional settings include:

```text
VESSEL_FL_ROUNDS=3
VESSEL_FL_LOCAL_EPOCHS=1
VESSEL_FL_BATCH_SIZE=16
VESSEL_FL_LEARNING_RATE=0.03
VESSEL_FL_TORCH_THREADS=2
VESSEL_FL_ROUND_TIMEOUT_SECONDS=180
VESSEL_FL_AUTO_START=false
VESSEL_FL_DATA_PARENT=~/Datasets/croatia-postprocessed
```

Roles are fixed at process startup. The deployment launches one coordinator,
three clients, one MLflow bridge, the MLflow server, and `ws-server`. MLflow
binds to `127.0.0.1` by default. Deliberately set `VESSEL_FL_MLFLOW_HOST=0.0.0.0`
only when LAN access is required, and do not expose it to an untrusted network
without authentication or a proxy.
