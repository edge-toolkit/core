// et_ws_pyface1.js - Browser adapter for the Pyodide workflow.
// Interface: default(), run(), start(), stop(), is_running()

const PYODIDE_BASE_URL = "/modules/pyodide/";

let pyodide;
let py;
let cfg;
let runtime = null;
let workCanvas = null;
let tensorData = null;

export default async function init() {
  if (!globalThis.loadPyodide) {
    await new Promise((resolve, reject) => {
      const script = document.createElement("script");
      script.src = `${PYODIDE_BASE_URL}pyodide.js`;
      script.onload = resolve;
      script.onerror = reject;
      document.head.appendChild(script);
    });
  }

  pyodide = await globalThis.loadPyodide({ indexURL: PYODIDE_BASE_URL });
  const pkg = await fetch(new URL("package.json", import.meta.url)).then((r) => r.json());
  const wheel = `${pkg.name.replace(/-/g, "_")}-${pkg.version}-py3-none-any.whl`;
  const wheelBytes = new Uint8Array(await fetch(new URL(wheel, import.meta.url)).then((r) => r.arrayBuffer()));
  pyodide.FS.writeFile(`/tmp/${wheel}`, wheelBytes);
  pyodide.runPython(`import sys\nsys.path.insert(0, "/tmp/${wheel}")`);
  py = pyodide.pyimport("pyface1");
  cfg = py.config().toJs({ dict_converter: Object.fromEntries });
}

export const is_running = () => runtime !== null;
export const start = () => run();

export async function run() {
  if (!py) throw new Error("pyface1: not initialized");
  if (runtime) return;

  setStatus(py.starting_status());
  log(py.model_log_message());

  let client = null;
  let stream = null;
  let state = null;

  try {
    const { WsClient, WsClientConfig } = await import("/modules/et-ws-wasm-agent/et_ws_wasm_agent.js");
    const protocol = window.location.protocol === "https:" ? "wss:" : "ws:";
    client = new WsClient(new WsClientConfig(`${protocol}//${window.location.host}/ws`));
    client.connect();
    for (let i = 0; client.get_state() !== "connected" && i < 100; i++) await sleep(100);
    if (client.get_state() !== "connected") throw new Error("Timed out waiting for websocket connection");
    log(`websocket connected with agent_id=${client.get_client_id()}`);

    stream = await navigator.mediaDevices.getUserMedia({ audio: false, video: true });
    const video = element("video-preview", HTMLVideoElement);
    video.srcObject = stream;
    video.hidden = false;
    for (let i = 0; video.videoWidth === 0 && i < 50; i++) await sleep(100);
    if (video.videoWidth === 0 || video.videoHeight === 0) throw new Error("Video stream metadata did not load");
    await video.play();

    const wasm = globalThis.ort?.env?.wasm;
    const version = globalThis.ort?.env?.versions?.web;
    if (!wasm || !version) throw new Error("onnxruntime-web environment is unavailable");
    const base = "/modules/onnxruntime-web/dist";
    wasm.numThreads = globalThis.crossOriginIsolated && globalThis.SharedArrayBuffer ? 0 : 1;
    wasm.wasmPaths = { mjs: `${base}/ort-wasm-simd-threaded.mjs`, wasm: `${base}/ort-wasm-simd-threaded.wasm` };

    const session = await globalThis.ort.InferenceSession.create(cfg.model_path, { executionProviders: ["wasm"] });
    const outputNames = py.validate_output_names(pyodide.toPy(Array.from(session.outputNames))).toJs();
    state = { client, stream, session, inputName: session.inputNames[0], outputNames };
    runtime = state;

    await py.run(
      state.inputName,
      pyodide.toPy(outputNames),
      pyodide.toPy(() => infer(state)),
      pyodide.toPy((message) => client.send(message)),
      pyodide.toPy(render),
      pyodide.toPy(sleep),
      pyodide.toPy(log),
      pyodide.toPy(setStatus),
      pyodide.toPy(() => runtime !== state),
    );
  } finally {
    cleanup(state ?? { client, stream });
  }
}

export function stop() {
  if (!runtime) return;
  cleanup(runtime);
  log("pyface1 face detection demo stopped");
}

async function infer(state) {
  const video = element("video-preview", HTMLVideoElement);
  if (video.videoWidth <= 0 || video.videoHeight <= 0) throw new Error("Video stream is not ready yet.");

  const geometry = py.preprocess_geometry(video.videoWidth, video.videoHeight).toJs({
    dict_converter: Object.fromEntries,
  });
  const canvas = workCanvas ??= document.createElement("canvas");
  canvas.width = cfg.input_width;
  canvas.height = cfg.input_height;

  const ctx = canvas.getContext("2d");
  ctx.clearRect(0, 0, canvas.width, canvas.height);
  ctx.drawImage(video, 0, 0, geometry.resized_width, geometry.resized_height);

  const tensor = imageDataToTensor(ctx.getImageData(0, 0, canvas.width, canvas.height).data);
  const outputs = await state.session.run({
    [state.inputName]: new globalThis.ort.Tensor("float32", tensor, [
      1,
      cfg.input_height,
      cfg.input_width,
      3,
    ]),
  });

  return pyodide.toPy({
    loc: Array.from(outputs[state.outputNames[0]].data),
    conf: Array.from(outputs[state.outputNames[1]].data),
    landm: Array.from(outputs[state.outputNames[2]].data),
    resize_ratio: geometry.resize_ratio,
    source_width: video.videoWidth,
    source_height: video.videoHeight,
  });
}

function render(detectionsJson) {
  const video = element("video-preview", HTMLVideoElement);
  if (video.videoWidth === 0 || video.videoHeight === 0) return;

  const canvas = element("video-output-canvas", HTMLCanvasElement);
  const ctx = canvas.getContext("2d");
  canvas.width = video.videoWidth;
  canvas.height = video.videoHeight;
  canvas.hidden = false;
  ctx.drawImage(video, 0, 0, canvas.width, canvas.height);
  ctx.lineWidth = 3;
  ctx.font = "16px ui-monospace, monospace";

  for (const detection of JSON.parse(detectionsJson)) {
    const [left, top, right, bottom] = detection.box;
    const label = `${detection.label} ${(detection.score * 100).toFixed(1)}%`;
    ctx.strokeStyle = "#ef8f35";
    ctx.strokeRect(left, top, Math.max(right - left, 1), Math.max(bottom - top, 1));
    ctx.fillStyle = "#182028";
    ctx.fillRect(left, Math.max(top - 24, 0), ctx.measureText(label).width + 10, 22);
    ctx.fillStyle = "#fffdfa";
    ctx.fillText(label, left + 5, Math.max(top - 8, 16));
  }
}

function cleanup(state) {
  if (runtime === state) runtime = null;
  for (const track of state?.stream?.getTracks?.() ?? []) track.stop();
  state?.client?.disconnect?.();

  const video = document.getElementById("video-preview");
  if (video) {
    video.pause();
    video.srcObject = null;
    video.hidden = true;
  }

  const canvas = document.getElementById("video-output-canvas");
  if (canvas) {
    canvas.hidden = true;
    canvas.getContext("2d")?.clearRect(0, 0, canvas.width, canvas.height);
  }
}

function setStatus(message) {
  const element = document.getElementById("module-output");
  if (element) element.value = message;
}

function log(message) {
  const line = `[pyface1] ${message}`;
  console.log(line);
  const element = document.getElementById("log");
  if (element) element.textContent = element.textContent ? `${element.textContent}\n${line}` : line;
}

function sleep(ms) {
  return new Promise((resolve) => window.setTimeout(resolve, ms));
}

function imageDataToTensor(rgba) {
  const pixelCount = cfg.input_width * cfg.input_height;
  if (!tensorData || tensorData.length !== pixelCount * 3) {
    tensorData = new Float32Array(pixelCount * 3);
  }

  for (let pixel = 0; pixel < pixelCount; pixel++) {
    const rgbaIndex = pixel * 4;
    const tensorIndex = pixel * 3;
    tensorData[tensorIndex] = rgba[rgbaIndex + 2] - 104;
    tensorData[tensorIndex + 1] = rgba[rgbaIndex + 1] - 117;
    tensorData[tensorIndex + 2] = rgba[rgbaIndex] - 123;
  }

  return tensorData;
}

function element(id, type) {
  const found = document.getElementById(id);
  if (!(found instanceof type)) throw new Error(`Missing #${id} element`);
  return found;
}
