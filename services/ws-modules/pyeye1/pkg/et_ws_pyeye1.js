// et_ws_pyeye1.js - Browser adapter for the MediaPipe FaceLandmarker eye-movement Pyodide workflow.
// Interface: default(), run(), start(), stop(), is_running()
//
// MediaPipe FaceLandmarker (a maintained, offline .task bundle that internally runs a face detector then a
// mesh model) does the inference in JS; the Python layer turns its normalized landmarks into eye boxes,
// iris circles, and the eye-misalignment / rhythmic-oscillation screening indicators this adapter renders.

const PYODIDE_BASE_PATH = "/modules/pyodide/";

// Stage-2 eye-box outline colours, keyed by the label the Python decoder emits.
const EYE_COLORS = {
  left_eye: "#ff4d4d",
  right_eye: "#4dff88",
};

let pyodide;
let py;
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
}

export const is_running = () => runtime !== null;
export const start = () => run();

export async function run() {
  if (!py) throw new Error("pyeye1: not initialized");
  if (runtime) return;

  const state = { client: null, stream: null, landmarker: null };
  runtime = state;
  try {
    await py.run(platformFor(state));
  } catch (err) {
    log(`pyeye1 run failed: ${String(err)}`);
    throw err;
  } finally {
    if (globalThis.__etPyCov) await globalThis.__etPyCov.stop(pyodide, "pyeye1");
    cleanup(state);
  }
}

// The Python workflow controls everything; each primitive here is one browser operation with no sequencing,
// polling, or timeout logic (Python owns those). Members mirror the contract documented on pyeye1's run().
function platformFor(state) {
  return {
    connect_ws: async () => {
      const wasmAgent = await import("/modules/et-ws-wasm-agent/et_ws_wasm_agent.js");
      await wasmAgent.default();
      const { WsClient, WsClientConfig } = wasmAgent;
      const protocol = window.location.protocol === "https:" ? "wss:" : "ws:";
      state.client = new WsClient(new WsClientConfig(`${protocol}//${window.location.host}/ws`));
      state.client.connect();
    },
    ws_state: () => state.client?.get_state() ?? "disconnected",
    agent_id: () => state.client?.get_agent_id(),
    send_event: (message) => state.client.send(message),
    start_camera: async () => {
      state.stream = await navigator.mediaDevices.getUserMedia({ audio: false, video: true });
      const video = element("video-preview", HTMLVideoElement);
      video.srcObject = state.stream;
      // The raw stream stays hidden; it still feeds the landmarker, and the canvas shows the cropped view.
      video.hidden = true;
    },
    video_size: () => {
      const video = element("video-preview", HTMLVideoElement);
      return [video.videoWidth, video.videoHeight];
    },
    play_video: () => element("video-preview", HTMLVideoElement).play(),
    load_landmarker: async (modelPath, bundlePath, wasmPath) => {
      // Load the MediaPipe tasks-vision runtime (served offline from its module mount) and build the
      // landmarker; Python passes the asset paths so the configuration stays on the Python side.
      const vision = await import(bundlePath);
      const fileset = await vision.FilesetResolver.forVisionTasks(wasmPath);
      state.landmarker = await vision.FaceLandmarker.createFromOptions(fileset, {
        baseOptions: { modelAssetPath: modelPath },
        runningMode: "VIDEO",
        numFaces: 1,
      });
    },
    infer: () => inferEyes(state),
    render,
    sleep,
    log,
    set_status: setStatus,
    should_stop: () => runtime !== state,
    cleanup: () => cleanup(state),
  };
}

export function stop() {
  if (!runtime) return;
  cleanup(runtime);
  log("pyeye1 eye movement screening demo stopped");
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

  const payload = JSON.parse(resultsJson);
  // Python decides the visible region (the face band around the eyes); before a face is seen the crop is
  // null and the full frame shows. Overlay coordinates stay in source pixels; the scale+translate maps them.
  const [cropLeft, cropTop, cropRight, cropBottom] = payload.crop ?? [0, 0, video.videoWidth, video.videoHeight];
  const cropWidth = Math.max(cropRight - cropLeft, 1);
  const cropHeight = Math.max(cropBottom - cropTop, 1);

  // Zoom the cropped band to the page width: the canvas element fills its container, and its backing store
  // matches the displayed size so the upscaled video stays as sharp as the source allows.
  const canvas = element("video-output-canvas", HTMLCanvasElement);
  canvas.hidden = false;
  canvas.style.width = "100%";
  canvas.style.height = "auto";
  const displayWidth = Math.max(canvas.clientWidth, 1);
  const scale = displayWidth / cropWidth;
  canvas.width = displayWidth;
  canvas.height = Math.max(Math.round(cropHeight * scale), 1);

  const ctx = canvas.getContext("2d");
  ctx.drawImage(video, cropLeft, cropTop, cropWidth, cropHeight, 0, 0, canvas.width, canvas.height);

  ctx.save();
  ctx.scale(scale, scale);
  ctx.translate(-cropLeft, -cropTop);
  // Divide widths/sizes by the zoom so strokes and labels keep a constant on-screen weight.
  ctx.font = `${16 / scale}px ui-monospace, monospace`;
  for (const result of payload.faces ?? []) {
    ctx.lineWidth = 3 / scale;
    for (const eye of result.eyes) {
      const [left, top, right, bottom] = eye.box;
      const color = EYE_COLORS[eye.label] ?? "#fffdfa";
      ctx.strokeStyle = color;
      ctx.strokeRect(left, top, Math.max(right - left, 1), Math.max(bottom - top, 1));
      ctx.fillStyle = color;
      ctx.fillText(eye.label === "left_eye" ? "L" : "R", left, Math.max(top - 4, 12));
    }

    ctx.lineWidth = 2 / scale;
    for (const iris of result.irises ?? []) {
      const [centerX, centerY] = iris.center;
      ctx.strokeStyle = EYE_COLORS[iris.label] ?? "#fffdfa";
      ctx.beginPath();
      ctx.arc(centerX, centerY, Math.max(iris.radius, 1), 0, 2 * Math.PI);
      ctx.stroke();
    }
  }
  ctx.restore();
  ctx.font = "16px ui-monospace, monospace";
  renderAnalysis(ctx, payload.analysis);
}

// Screening-verdict overlay, top-left. Python sends analysis=null until the first window completes, and each
// screening reports status "insufficient_data" until it has enough non-blink samples to rate.
function renderAnalysis(ctx, analysis) {
  const screenings = [
    ["eye misalignment", analysis?.misalignment],
    ["rhythmic oscillation", analysis?.oscillation],
  ];
  const lines = [];
  for (const [name, metrics] of screenings) {
    if (metrics?.status === "ok") lines.push(`${name}: ${metrics.detected ? "DETECTED" : "none"}`);
  }
  if (lines.length > 0) lines.push("screening demo -- not a medical diagnosis");
  lines.forEach((line, index) => {
    ctx.fillStyle = line.includes("DETECTED") ? "#ffb84d" : "#d7e0e8";
    ctx.fillText(line, 8, 20 + index * 20);
  });
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
