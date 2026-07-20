// et_ws_pyeye1.js - Browser adapter for the MediaPipe FaceLandmarker eye-detection Pyodide workflow.
// Interface: default(), run(), start(), stop(), is_running()
//
// MediaPipe FaceLandmarker (a maintained, offline .task bundle that internally runs a face detector then a
// mesh model) does the inference in JS; the Python layer turns its normalized landmarks into eye boxes.

const PYODIDE_BASE_PATH = "/modules/pyodide/";

// Stage-2 eye-box outline colours, keyed by the label the Python decoder emits.
const EYE_COLORS = {
  left_eye: "#ff4d4d",
  right_eye: "#4dff88",
};

let pyodide;
let py;
let cfg;
let runtime = null;

export default async function init() {
  if (!globalThis.loadPyodide) {
    await new Promise((resolve, reject) => {
      const script = document.createElement("script");
      script.src = `${PYODIDE_BASE_PATH}pyodide.js`;
      script.onload = resolve;
      script.onerror = reject;
      document.head.appendChild(script);
    });
  }

  pyodide = await globalThis.loadPyodide({ indexURL: PYODIDE_BASE_PATH });

  // pydantic backs the typed WsClientEvent message in the et_ws wheel.
  await pyodide.loadPackage("micropip");
  const micropip = pyodide.pyimport("micropip");
  await micropip.install("pydantic");

  // Install pyeye1's own wheel from pkg/ next to this shim.
  const installLocalWheel = async (path) => {
    const bytes = new Uint8Array(await fetch(new URL(path, import.meta.url)).then((r) => r.arrayBuffer()));
    pyodide.FS.writeFile(`/tmp/${path}`, bytes);
    pyodide.runPython(`import sys\nsys.path.insert(0, "/tmp/${path}")`);
  };

  const pkg = await fetch(new URL("package.json", import.meta.url)).then((r) => r.json());
  await installLocalWheel(`${pkg.name.replace(/-/g, "_")}-${pkg.version}-py3-none-any.whl`);
  // et-ws is its own ws-module mounted at /modules/et-ws/; delegate its wheel install to its shim.
  const { installWheel: installEtWs } = await import("/modules/et-ws/et_ws.js");
  await installEtWs(pyodide);

  if (globalThis.__etPyCov) await globalThis.__etPyCov.start(pyodide, "pyeye1");
  py = pyodide.pyimport("pyeye1");
  cfg = py.config().toJs({ dict_converter: Object.fromEntries });
}

export const is_running = () => runtime !== null;
export const start = () => run();

export async function run() {
  if (!py) throw new Error("pyeye1: not initialized");
  if (runtime) return;

  setStatus(py.starting_status());
  log(py.model_log_message());

  let client = null;
  let stream = null;
  let state = null;

  try {
    const wasmAgent = await import("/modules/et-ws-wasm-agent/et_ws_wasm_agent.js");
    await wasmAgent.default();
    const { WsClient, WsClientConfig } = wasmAgent;
    const protocol = window.location.protocol === "https:" ? "wss:" : "ws:";
    client = new WsClient(new WsClientConfig(`${protocol}//${window.location.host}/ws`));
    client.connect();
    for (let i = 0; client.get_state() !== "connected" && i < 100; i++) await sleep(100);
    if (client.get_state() !== "connected") throw new Error("Timed out waiting for websocket connection");
    log(`websocket connected with agent_id=${client.get_agent_id()}`);

    stream = await navigator.mediaDevices.getUserMedia({ audio: false, video: true });
    const video = element("video-preview", HTMLVideoElement);
    video.srcObject = stream;
    video.hidden = false;
    for (let i = 0; video.videoWidth === 0 && i < 50; i++) await sleep(100);
    if (video.videoWidth === 0 || video.videoHeight === 0) throw new Error("Video stream metadata did not load");
    await video.play();

    // Load the MediaPipe tasks-vision runtime (served offline from its module mount) and build the landmarker.
    const vision = await import(cfg.bundle_path);
    const { FaceLandmarker, FilesetResolver } = vision;
    const fileset = await FilesetResolver.forVisionTasks(cfg.wasm_path);
    const landmarker = await FaceLandmarker.createFromOptions(fileset, {
      baseOptions: { modelAssetPath: cfg.model_path },
      runningMode: "VIDEO",
      numFaces: 1,
    });

    state = { client, stream, landmarker };
    runtime = state;

    await py.run(
      pyodide.toPy(() => inferEyes(state)),
      pyodide.toPy((message) => client.send(message)),
      pyodide.toPy(render),
      pyodide.toPy(sleep),
      pyodide.toPy(log),
      pyodide.toPy(setStatus),
      pyodide.toPy(() => runtime !== state),
    );
  } catch (err) {
    log(`pyeye1 run failed: ${String(err)}`);
    throw err;
  } finally {
    if (globalThis.__etPyCov) await globalThis.__etPyCov.stop(pyodide, "pyeye1");
    cleanup(state ?? { client, stream });
  }
}

export function stop() {
  if (!runtime) return;
  cleanup(runtime);
  log("pyeye1 eye detection demo stopped");
}

async function inferEyes(state) {
  const video = element("video-preview", HTMLVideoElement);
  if (video.videoWidth <= 0 || video.videoHeight <= 0) throw new Error("Video stream is not ready yet.");

  const result = state.landmarker.detectForVideo(video, performance.now());
  const faces = (result.faceLandmarks ?? []).map((face) => {
    const flat = [];
    for (const point of face) flat.push(point.x, point.y);
    return flat;
  });

  return JSON.stringify({ faces, width: video.videoWidth, height: video.videoHeight });
}

function render(resultsJson) {
  const video = element("video-preview", HTMLVideoElement);
  if (video.videoWidth === 0 || video.videoHeight === 0) return;

  const canvas = element("video-output-canvas", HTMLCanvasElement);
  const ctx = canvas.getContext("2d");
  canvas.width = video.videoWidth;
  canvas.height = video.videoHeight;
  canvas.hidden = false;
  ctx.drawImage(video, 0, 0, canvas.width, canvas.height);
  ctx.font = "16px ui-monospace, monospace";

  for (const result of JSON.parse(resultsJson)) {
    const [faceLeft, faceTop, faceRight, faceBottom] = result.face_box;
    ctx.lineWidth = 1;
    ctx.strokeStyle = "#7a8794";
    ctx.strokeRect(faceLeft, faceTop, Math.max(faceRight - faceLeft, 1), Math.max(faceBottom - faceTop, 1));

    ctx.lineWidth = 3;
    for (const eye of result.eyes) {
      const [left, top, right, bottom] = eye.box;
      const color = EYE_COLORS[eye.label] ?? "#fffdfa";
      ctx.strokeStyle = color;
      ctx.strokeRect(left, top, Math.max(right - left, 1), Math.max(bottom - top, 1));
      ctx.fillStyle = color;
      ctx.fillText(eye.label === "left_eye" ? "L" : "R", left, Math.max(top - 4, 12));
    }
  }
}

function cleanup(state) {
  if (runtime === state) runtime = null;
  state?.landmarker?.close?.();
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
  const output = document.getElementById("module-output");
  if (output) output.value = message;
}

function log(message) {
  const line = `[pyeye1] ${message}`;
  console.log(line);
  const logEl = document.getElementById("log");
  if (logEl) logEl.textContent = logEl.textContent ? `${logEl.textContent}\n${line}` : line;
}

function sleep(ms) {
  return new Promise((resolve) => window.setTimeout(resolve, ms));
}

function element(id, type) {
  const found = document.getElementById(id);
  if (!(found instanceof type)) throw new Error(`Missing #${id} element`);
  return found;
}
