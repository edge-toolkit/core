// Browser adapter: microphone capture + ONNX inference for the Python workflow.
const PYODIDE_BASE_PATH = "/modules/pyodide/";

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
  await pyodide.loadPackage("micropip");
  await pyodide.pyimport("micropip").install("pydantic");

  const installLocalWheel = async (path) => {
    const response = await fetch(new URL(path, import.meta.url));
    if (!response.ok) throw new Error(`Unable to fetch ${path}: HTTP ${response.status}`);
    pyodide.FS.writeFile(`/tmp/${path}`, new Uint8Array(await response.arrayBuffer()));
    pyodide.runPython(`import sys\nsys.path.insert(0, "/tmp/${path}")`);
  };

  const pkg = await fetch(new URL("package.json", import.meta.url)).then((response) => response.json());
  await installLocalWheel(`${pkg.name.replace(/-/g, "_")}-${pkg.version}-py3-none-any.whl`);
  const { installWheel: installEtWs } = await import("/modules/et-ws/et_ws.js");
  await installEtWs(pyodide);
  if (globalThis.__etPyCov) await globalThis.__etPyCov.start(pyodide, "pyspeech1");
  py = pyodide.pyimport("pyspeech1");
  cfg = py.config().toJs({ dict_converter: Object.fromEntries });
}

export const is_running = () => runtime !== null;
export const start = () => run();

export async function run() {
  if (!py) throw new Error("pyspeech1: not initialized");
  if (runtime) return;

  setStatus("pyspeech1 speech detection ready. Press Record audio to begin.");
  log(py.model_log_message());
  const state = {
    client: null,
    stream: null,
    audioContext: null,
    session: null,
    stopped: false,
    busy: false,
    recordButton: null,
    visualization: null,
  };
  runtime = state;

  try {
    const wasmAgent = await import("/modules/et-ws-wasm-agent/et_ws_wasm_agent.js");
    await wasmAgent.default();
    const protocol = window.location.protocol === "https:" ? "wss:" : "ws:";
    state.client = new wasmAgent.WsClient(new wasmAgent.WsClientConfig(`${protocol}//${window.location.host}/ws`));
    state.client.connect();
    for (let i = 0; state.client.get_state() !== "connected" && i < 100; i++) await sleep(100);
    if (state.client.get_state() !== "connected") throw new Error("Timed out waiting for websocket connection");

    configureOnnxRuntime();
    state.session = await globalThis.ort.InferenceSession.create(cfg.model_path, {
      executionProviders: ["wasm"],
    });
    validateModelInterface(state.session);
    state.recordButton = createRecordingControls(state);
    log("ready; waiting for the user to start recording");
    if (globalThis.__etPyCov) await globalThis.__etPyCov.stop(pyodide, "pyspeech1");
  } catch (error) {
    if (!state.stopped) {
      setStatus(`pyspeech1 speech detection failed\n${String(error)}`);
      log(`run failed: ${String(error)}`);
      cleanup(state);
      throw error;
    }
  }
}

export function stop() {
  if (!runtime) return;
  const state = runtime;
  state.stopped = true;
  cleanup(state);
  setStatus(py?.stopped_status?.() ?? "pyspeech1 speech detection stopped.");
}

async function captureAndInfer(state) {
  state.stream = await navigator.mediaDevices.getUserMedia({ audio: true, video: false });
  const AudioContext = globalThis.AudioContext ?? globalThis.webkitAudioContext;
  if (!AudioContext) throw new Error("Web Audio is unavailable");
  state.audioContext = new AudioContext();
  await state.audioContext.resume();

  const sourceRate = state.audioContext.sampleRate;
  const source = state.audioContext.createMediaStreamSource(state.stream);
  const processor = state.audioContext.createScriptProcessor(4096, 1, 1);
  const sink = state.audioContext.createGain();
  sink.gain.value = 0;
  const blocks = [];
  state.visualization = createWaveform(cfg.capture_seconds);
  processor.onaudioprocess = (event) => {
    const block = new Float32Array(event.inputBuffer.getChannelData(0));
    blocks.push(block);
    addWaveformSamples(state.visualization, block);
  };
  source.connect(processor);
  processor.connect(sink);
  sink.connect(state.audioContext.destination);

  const started = performance.now();
  try {
    await sleep(cfg.capture_seconds * 1000);
  } finally {
    processor.disconnect();
    source.disconnect();
    sink.disconnect();
    for (const track of state.stream?.getTracks?.() ?? []) track.stop();
    state.stream = null;
    await state.audioContext?.close?.();
    state.audioContext = null;
  }
  if (state.stopped) throw new Error("capture stopped");

  const recordedSeconds = (performance.now() - started) / 1000;
  finishWaveform(state.visualization, "ANALYZING");
  const audio = resample(concatenate(blocks), sourceRate, cfg.sample_rate);
  const probabilities = await inferProbabilities(state.session, audio);
  return pyodide.toPy({ probabilities, source_sample_rate: sourceRate, recorded_seconds: recordedSeconds });
}

async function recordAndDetect(state) {
  if (state.busy || state.stopped || runtime !== state) return;
  state.busy = true;
  const button = state.recordButton;
  button.disabled = true;
  button.style.cursor = "wait";
  button.style.opacity = "0.72";
  button.dataset.state = "recording";
  button.querySelector("span:last-child").textContent = "Recording…";
  setStatus(py.starting_status());

  try {
    await py.run(
      pyodide.toPy(() => captureAndInfer(state)),
      pyodide.toPy((message) => state.client.send(message)),
      pyodide.toPy((detected, confidence) => showDetectionResult(state.visualization, detected, confidence)),
      pyodide.toPy(log),
      pyodide.toPy(setStatus),
    );
    finishWaveform(state.visualization, "COMPLETE");
    button.querySelector("span:last-child").textContent = "Record again";
  } catch (error) {
    if (!state.stopped) {
      finishWaveform(state.visualization, "ERROR");
      setStatus(`pyspeech1 speech detection failed\n${String(error)}`);
      log(`recording failed: ${String(error)}`);
      button.querySelector("span:last-child").textContent = "Try again";
    }
  } finally {
    state.busy = false;
    if (!state.stopped) {
      button.disabled = false;
      button.style.cursor = "pointer";
      button.style.opacity = "1";
      button.dataset.state = "ready";
    }
  }
}

function createRecordingControls(state) {
  document.getElementById("pyspeech1-recording-controls")?.remove();
  const canvas = document.getElementById("video-output-canvas");
  if (!(canvas instanceof HTMLCanvasElement)) throw new Error("Missing waveform canvas");

  const controls = document.createElement("div");
  controls.id = "pyspeech1-recording-controls";
  const controlStyles = [
    "display:flex",
    "align-items:center",
    "justify-content:space-between",
    "gap:18px",
    "margin:18px auto 0",
    "padding:16px 18px",
    "max-width:720px",
    "box-sizing:border-box",
    "border:1px solid rgba(24,32,40,.13)",
    "border-radius:16px",
    "background:linear-gradient(135deg,rgba(255,255,255,.9),rgba(239,248,246,.88))",
    "box-shadow:0 12px 34px rgba(24,32,40,.10)",
  ];
  controls.style.cssText = controlStyles.join(";");

  const copy = document.createElement("div");
  const title = document.createElement("strong");
  title.style.cssText = "display:block;font:700 15px ui-monospace,monospace;color:#182028";
  title.textContent = "Speech sample";
  const description = document.createElement("span");
  description.style.cssText = "display:block;margin-top:4px;font:13px ui-monospace,monospace;color:#64747a";
  description.textContent = `Capture a ${cfg.capture_seconds}-second clip for local analysis`;
  copy.append(title, description);

  const button = document.createElement("button");
  button.type = "button";
  button.dataset.state = "ready";
  button.setAttribute("aria-label", "Record audio for speech detection");
  const buttonStyles = [
    "display:inline-flex",
    "align-items:center",
    "gap:10px",
    "min-width:158px",
    "justify-content:center",
    "padding:12px 18px",
    "border:0",
    "border-radius:999px",
    "background:linear-gradient(135deg,#102d3b,#176454)",
    "color:#f4fffc",
    "font:700 14px ui-monospace,monospace",
    "letter-spacing:.01em",
    "cursor:pointer",
    "box-shadow:0 8px 20px rgba(16,83,72,.24)",
    "transition:transform .15s ease,box-shadow .15s ease,opacity .15s ease",
  ];
  button.style.cssText = buttonStyles.join(";");
  const recordingIndicator = document.createElement("span");
  recordingIndicator.setAttribute("aria-hidden", "true");
  const indicatorStyles = [
    "width:10px",
    "height:10px",
    "border-radius:50%",
    "background:#ff6577",
    "box-shadow:0 0 0 4px rgba(255,101,119,.16)",
  ];
  recordingIndicator.style.cssText = indicatorStyles.join(";");
  const buttonLabel = document.createElement("span");
  buttonLabel.textContent = "Record audio";
  button.append(recordingIndicator, buttonLabel);
  button.addEventListener("mouseenter", () => {
    if (!button.disabled) button.style.transform = "translateY(-1px)";
  });
  button.addEventListener("mouseleave", () => {
    button.style.transform = "none";
  });
  button.addEventListener("click", () => recordAndDetect(state));

  controls.append(copy, button);
  canvas.before(controls);
  return button;
}

async function inferProbabilities(session, samples) {
  let recurrentState = new Float32Array(2 * 128);
  let context = new Float32Array(cfg.context_size);
  const probabilities = [];

  for (let offset = 0; offset < samples.length; offset += cfg.chunk_size) {
    const input = new Float32Array(cfg.context_size + cfg.chunk_size);
    input.set(context);
    input.set(samples.subarray(offset, Math.min(offset + cfg.chunk_size, samples.length)), cfg.context_size);
    const outputs = await session.run({
      input: new globalThis.ort.Tensor("float32", input, [1, input.length]),
      state: new globalThis.ort.Tensor("float32", recurrentState, [2, 1, 128]),
      sr: new globalThis.ort.Tensor("int64", BigInt64Array.of(BigInt(cfg.sample_rate)), []),
    });
    probabilities.push(Number(outputs.output.data[0]));
    recurrentState = new Float32Array(outputs.stateN.data);
    context = input.slice(input.length - cfg.context_size);
  }
  return probabilities;
}

function configureOnnxRuntime() {
  const wasm = globalThis.ort?.env?.wasm;
  const version = globalThis.ort?.env?.versions?.web;
  if (!wasm || !version) throw new Error("onnxruntime-web environment is unavailable");
  const base = "/modules/onnxruntime-web/dist";
  wasm.numThreads = globalThis.crossOriginIsolated && globalThis.SharedArrayBuffer ? 0 : 1;
  wasm.wasmPaths = { mjs: `${base}/ort-wasm-simd-threaded.mjs`, wasm: `${base}/ort-wasm-simd-threaded.wasm` };
}

function validateModelInterface(session) {
  for (const name of ["input", "state", "sr"]) {
    if (!session.inputNames.includes(name)) throw new Error(`Speech detection model is missing input ${name}`);
  }
  for (const name of ["output", "stateN"]) {
    if (!session.outputNames.includes(name)) throw new Error(`Speech detection model is missing output ${name}`);
  }
}

function concatenate(blocks) {
  const length = blocks.reduce((total, block) => total + block.length, 0);
  const result = new Float32Array(length);
  let offset = 0;
  for (const block of blocks) {
    result.set(block, offset);
    offset += block.length;
  }
  return result;
}

function resample(input, sourceRate, targetRate) {
  if (sourceRate === targetRate) return input;
  const output = new Float32Array(Math.max(1, Math.round((input.length * targetRate) / sourceRate)));
  const ratio = sourceRate / targetRate;
  for (let i = 0; i < output.length; i++) {
    const position = i * ratio;
    const left = Math.min(Math.floor(position), input.length - 1);
    const right = Math.min(left + 1, input.length - 1);
    const fraction = position - left;
    output[i] = input[left] * (1 - fraction) + input[right] * fraction;
  }
  return output;
}

function cleanup(state) {
  if (runtime === state) runtime = null;
  if (state.visualization?.animationFrame) cancelAnimationFrame(state.visualization.animationFrame);
  for (const track of state.stream?.getTracks?.() ?? []) track.stop();
  state.audioContext?.close?.();
  state.client?.disconnect?.();
  document.getElementById("pyspeech1-recording-controls")?.remove();
  const canvas = document.getElementById("video-output-canvas");
  if (canvas) canvas.hidden = true;
}

function createWaveform(durationSeconds) {
  const canvas = document.getElementById("video-output-canvas");
  if (!(canvas instanceof HTMLCanvasElement)) throw new Error("Missing waveform canvas");
  canvas.width = 1440;
  canvas.height = 880;
  canvas.style.width = "100%";
  canvas.style.maxHeight = "none";
  canvas.hidden = false;
  const visualization = {
    canvas,
    context: canvas.getContext("2d"),
    durationSeconds,
    startedAt: performance.now(),
    peaks: [],
    level: 0,
    speechDetected: null,
    speechConfidence: 0,
    phase: "RECORDING",
    animationFrame: 0,
  };
  const animate = () => {
    drawWaveform(visualization);
    if (visualization.phase === "RECORDING") {
      visualization.animationFrame = requestAnimationFrame(animate);
    }
  };
  animate();
  return visualization;
}

function showDetectionResult(visualization, detected, confidence) {
  if (!visualization) return;
  visualization.speechDetected = Boolean(detected);
  visualization.speechConfidence = Number(confidence);
  drawWaveform(visualization);
}

function addWaveformSamples(visualization, samples) {
  if (!visualization) return;
  const bins = 8;
  const binSize = Math.max(1, Math.floor(samples.length / bins));
  let energy = 0;
  for (const sample of samples) energy += sample * sample;
  const rms = Math.sqrt(energy / Math.max(samples.length, 1));
  visualization.level += (rms - visualization.level) * 0.35;

  for (let bin = 0; bin < bins; bin++) {
    let low = 0;
    let high = 0;
    const end = Math.min((bin + 1) * binSize, samples.length);
    for (let index = bin * binSize; index < end; index++) {
      low = Math.min(low, samples[index]);
      high = Math.max(high, samples[index]);
    }
    visualization.peaks.push({ low, high });
  }
  if (visualization.peaks.length > 720) visualization.peaks.splice(0, visualization.peaks.length - 720);
}

function finishWaveform(visualization, phase) {
  if (!visualization) return;
  visualization.phase = phase;
  if (visualization.animationFrame) cancelAnimationFrame(visualization.animationFrame);
  visualization.animationFrame = 0;
  drawWaveform(visualization);
}

function drawWaveform(visualization) {
  const { canvas, context: ctx } = visualization;
  const width = canvas.width;
  const height = canvas.height;
  const elapsed = Math.min((performance.now() - visualization.startedAt) / 1000, visualization.durationSeconds);
  const progress = visualization.phase === "RECORDING" ? elapsed / visualization.durationSeconds : 1;

  const background = ctx.createLinearGradient(0, 0, width, height);
  background.addColorStop(0, "#091523");
  background.addColorStop(0.55, "#10253a");
  background.addColorStop(1, "#091923");
  ctx.fillStyle = background;
  ctx.fillRect(0, 0, width, height);

  const glow = ctx.createRadialGradient(width * progress, height * 0.55, 0, width * progress, height * 0.55, 430);
  glow.addColorStop(0, "rgba(37, 211, 180, 0.16)");
  glow.addColorStop(1, "rgba(37, 211, 180, 0)");
  ctx.fillStyle = glow;
  ctx.fillRect(0, 0, width, height);

  ctx.strokeStyle = "rgba(166, 211, 221, 0.075)";
  ctx.lineWidth = 1;
  for (let x = 48; x < width; x += 72) line(ctx, x, 92, x, height - 62);
  for (let y = 112; y < height - 52; y += 54) line(ctx, 44, y, width - 44, y);

  ctx.font = "700 40px ui-monospace, SFMono-Regular, Menlo, monospace";
  ctx.fillStyle = "#edf9f7";
  ctx.fillText("LIVE AUDIO", 48, 64);
  ctx.font = "600 27px ui-monospace, SFMono-Regular, Menlo, monospace";
  ctx.fillStyle = "#8da8b2";
  ctx.fillText("SPEECH DETECTION INPUT", 48, 103);

  const active = visualization.phase === "RECORDING";
  ctx.beginPath();
  ctx.arc(width - 300, 58, 10, 0, Math.PI * 2);
  ctx.fillStyle = active ? "#ff6174" : "#26d3b4";
  ctx.shadowColor = ctx.fillStyle;
  ctx.shadowBlur = 18;
  ctx.fill();
  ctx.shadowBlur = 0;
  ctx.fillStyle = active ? "#ffb0ba" : "#9af1de";
  ctx.fillText(visualization.phase, width - 275, 67);
  ctx.fillStyle = "#a9bec5";
  ctx.textAlign = "right";
  ctx.fillText(`${elapsed.toFixed(1)} / ${visualization.durationSeconds.toFixed(1)}s`, width - 48, 103);
  ctx.textAlign = "left";

  const center = height * 0.52;
  const plotLeft = 48;
  const plotWidth = width - 96;
  const peaks = visualization.peaks;
  if (peaks.length) {
    const gradient = ctx.createLinearGradient(plotLeft, 0, plotLeft + plotWidth, 0);
    gradient.addColorStop(0, "#47a9ff");
    gradient.addColorStop(0.5, "#33e1c1");
    gradient.addColorStop(1, "#b5f25d");
    ctx.strokeStyle = gradient;
    ctx.lineWidth = Math.max(2, (plotWidth / Math.max(peaks.length, 1)) * 0.66);
    ctx.lineCap = "round";
    ctx.shadowColor = "rgba(51, 225, 193, 0.55)";
    ctx.shadowBlur = 13;
    const scale = 285;
    for (let index = 0; index < peaks.length; index++) {
      const x = plotLeft + (index / Math.max(peaks.length - 1, 1)) * plotWidth * progress;
      const peak = peaks[index];
      line(ctx, x, center + peak.low * scale, x, center + peak.high * scale);
    }
    ctx.shadowBlur = 0;
  } else {
    ctx.strokeStyle = "rgba(51, 225, 193, 0.7)";
    line(ctx, plotLeft, center, plotLeft + plotWidth * progress, center);
  }

  if (visualization.speechDetected === true) {
    const badgeWidth = 520;
    const badgeHeight = 84;
    const badgeX = (width - badgeWidth) / 2;
    const badgeY = height - 190;
    ctx.shadowColor = "rgba(42, 231, 191, 0.38)";
    ctx.shadowBlur = 30;
    ctx.fillStyle = "rgba(15, 77, 67, 0.94)";
    roundedRect(ctx, badgeX, badgeY, badgeWidth, badgeHeight, 22);
    ctx.shadowBlur = 0;
    ctx.strokeStyle = "rgba(126, 245, 218, 0.72)";
    ctx.lineWidth = 2;
    ctx.beginPath();
    ctx.roundRect(badgeX, badgeY, badgeWidth, badgeHeight, 22);
    ctx.stroke();
    ctx.beginPath();
    ctx.arc(badgeX + 42, badgeY + badgeHeight / 2, 10, 0, Math.PI * 2);
    ctx.fillStyle = "#62f0cf";
    ctx.shadowColor = "#62f0cf";
    ctx.shadowBlur = 18;
    ctx.fill();
    ctx.shadowBlur = 0;
    ctx.fillStyle = "#eafffa";
    ctx.font = "800 34px ui-monospace, SFMono-Regular, Menlo, monospace";
    ctx.fillText("SPEECH DETECTED", badgeX + 70, badgeY + 43);
    ctx.fillStyle = "#9ee9d8";
    ctx.font = "600 19px ui-monospace, SFMono-Regular, Menlo, monospace";
    ctx.fillText(`${(visualization.speechConfidence * 100).toFixed(1)}% peak confidence`, badgeX + 72, badgeY + 68);
  }

  const progressY = height - 36;
  ctx.fillStyle = "rgba(255, 255, 255, 0.09)";
  roundedRect(ctx, 48, progressY, plotWidth, 7, 4);
  const progressGradient = ctx.createLinearGradient(48, 0, width - 48, 0);
  progressGradient.addColorStop(0, "#47a9ff");
  progressGradient.addColorStop(1, "#33e1c1");
  ctx.fillStyle = progressGradient;
  roundedRect(ctx, 48, progressY, plotWidth * progress, 7, 4);

  ctx.fillStyle = "#8da8b2";
  ctx.font = "600 27px ui-monospace, SFMono-Regular, Menlo, monospace";
  ctx.fillText("INPUT LEVEL", 48, height - 58);
  ctx.fillStyle = visualization.level > 0.025 ? "#7ce8cf" : "#78949e";
  ctx.fillText(visualization.level > 0.025 ? "VOICE ACTIVITY" : "LISTENING", 265, height - 58);
}

function line(ctx, x1, y1, x2, y2) {
  ctx.beginPath();
  ctx.moveTo(x1, y1);
  ctx.lineTo(x2, y2);
  ctx.stroke();
}

function roundedRect(ctx, x, y, width, height, radius) {
  if (width <= 0) return;
  ctx.beginPath();
  ctx.roundRect(x, y, width, height, radius);
  ctx.fill();
}

function setStatus(message) {
  const output = document.getElementById("module-output");
  if (output) output.value = message;
}

function log(message) {
  const logLine = `[pyspeech1] ${message}`;
  console.log(logLine);
  const output = document.getElementById("log");
  if (output) output.textContent = output.textContent ? `${output.textContent}\n${logLine}` : logLine;
}

function sleep(ms) {
  return new Promise((resolve) => window.setTimeout(resolve, ms));
}
