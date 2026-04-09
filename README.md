# edge-toolkit core

## mise

Please install [`mise`](https://mise.jdx.dev/).
It is needed for all use of this repository.

## Contributing

Use `mise run fmt` and `mise run check` to run formatters and checkers.

## Run e2e

Run the end-to-end tests using Chrome:

```bash
mise run ws-e2e-chrome
```

## Run ws agent in browser

### HAR model setup

Download the onnx from https://modelnova.ai/models/details/human-activity-recognition ,
and save it as `services/ws-server/static/models/human_activity_recognition.onnx`

### Face detection setup

Download the onnx from https://huggingface.co/amd/retinaface and save it in
`services/ws-server/static/models/` and rename the file to `video_cv.onnx`.

### Build and run the agent

```bash
mise run build-ws-wasm-agent
mise run build-ws-har1-module
mise run ws-server
```

The WASM build disables WebAssembly reference types so it can still load on older browsers such as Chrome 95.

Find the IP address of your laptop in the local network,
which will normally be something like 192.168.1.x.

Then on your phone, open Chrome and type in https://192.168.1.x:8433/

Click "har demo".

For webcam inference, click "Load video CV model" and then "Start video".

## Grant

This repository is part of a grant managed by the School of EECMS, Curtin University.

```text
ABN 99 143 842 569.

CRICOS Provider Code 00301J.

TEQSA PRV12158
```
