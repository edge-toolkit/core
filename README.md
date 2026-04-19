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

1. Download RetinaFace_int.onnx from https://huggingface.co/amd/retinaface/tree/main/weights
2. Save it in `services/ws-server/static/models/`
3. Rename the file to `video_cv.onnx`.

### Build WASM and run the WS server

In a separate terminal start OpenObserve (o2) and leave it running.

```bash
mise run o2
```

Then start the server

```bash
mise run build-wasm
mise run ws-server
```

Scan the QR-Code with a smart-phone camera and open the URL.

Select the module to run in the drop-down, then click "Run module" button.

The module list is dynamically populated from the modules in [services/ws-modules](services/ws-modules).

Note: The WASM build disables WebAssembly reference types, so it can still load on older browsers such as Chrome 95.

In a separate terminal, open the OpenObserve UX using:

```bash
mise run open-o2
```

The server logs appear in the Logs section.

## Grant

This repository is part of a grant managed by the School of EECMS, Curtin University.

```text
ABN 99 143 842 569.

CRICOS Provider Code 00301J.

TEQSA PRV12158
```
