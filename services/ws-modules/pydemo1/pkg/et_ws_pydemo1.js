// Full-screen browser adapter for the combined Python eye and speech demo.
const PYODIDE_BASE_PATH = "/modules/pyodide/";
// Both detections run for the same fixed window: the eye loop and the speech recording each last this long.
const CAPTURE_SECONDS = 30;

const EYE_COLORS = {
  left_eye: "#ff6f7d",
  right_eye: "#56e6b1",
};

let pyodide;
let py;
let cfg;
let runtime = null;
let pythonScriptPromise = null;
let pythonRuntimePromise = null;

export default async function init() {}

export const is_running = () => runtime !== null;
export const start = () => run();

export async function run() {
  if (runtime) return;

  const state = {
    activeDemo: null,
    client: null,
    popstateHandler: null,
    stopped: false,
  };
  runtime = state;
  log("launching the combined demo");
  await enterDemo(state);
}

export function stop() {
  if (!runtime) return;
  const state = runtime;
  state.stopped = true;
  if (state.activeDemo && window.history.state?.pydemo1) {
    window.history.back();
  } else {
    exitDemo(state);
  }
  runtime = null;
  setMainStatus("pydemo1 stopped.");
}

async function loadPythonRuntime() {
  if (py) return;
  if (pythonRuntimePromise) return pythonRuntimePromise;
  pythonRuntimePromise = initializePythonRuntime();
  try {
    await pythonRuntimePromise;
  } catch (error) {
    pythonRuntimePromise = null;
    throw error;
  }
}

async function initializePythonRuntime() {
  setPreparationStatus("Loading the Python runtime script...");
  await loadPythonRuntimeScript();

  setPreparationStatus("Starting the local Python interpreter...");
  pyodide = await globalThis.loadPyodide({ indexURL: PYODIDE_BASE_PATH });
  setPreparationStatus("Installing local Python dependencies...");
  await pyodide.loadPackage("micropip");
  await pyodide.pyimport("micropip").install("pydantic");
  setPreparationStatus("Loading the demo Python modules...");
  await Promise.all([
    installModuleWheel("/modules/et-ws-pyeye1/"),
    installModuleWheel("/modules/et-ws-pyspeech1/"),
    installModuleWheel(new URL(".", import.meta.url)),
  ]);
  setPreparationStatus("Installing the WebSocket Python support module...");
  const { installWheel: installEtWs } = await import("/modules/et-ws/et_ws.js");
  await installEtWs(pyodide);
  if (globalThis.__etPyCov) await globalThis.__etPyCov.start(pyodide, "pydemo1");
  try {
    setPreparationStatus("Importing the combined demo workflow...");
    py = pyodide.pyimport("pydemo1");
    cfg = py.config().toJs({ dict_converter: Object.fromEntries });
  } finally {
    if (globalThis.__etPyCov) await globalThis.__etPyCov.stop(pyodide, "pydemo1");
  }
}

async function loadPythonRuntimeScript() {
  if (globalThis.loadPyodide) return;
  if (pythonScriptPromise) return pythonScriptPromise;
  pythonScriptPromise = new Promise((resolve, reject) => {
    const script = document.createElement("script");
    script.src = `${PYODIDE_BASE_PATH}pyodide.js`;
    script.onload = resolve;
    script.onerror = reject;
    document.head.appendChild(script);
  });
  try {
    await pythonScriptPromise;
  } catch (error) {
    pythonScriptPromise = null;
    throw error;
  }
}

async function installModuleWheel(baseUrl) {
  const resolvedBaseUrl = baseUrl instanceof URL ? baseUrl : new URL(baseUrl, window.location.origin);
  const pkgUrl = new URL("package.json", resolvedBaseUrl);
  const pkg = await fetch(pkgUrl).then((response) => {
    if (!response.ok) throw new Error(`Unable to load ${pkgUrl}: HTTP ${response.status}`);
    return response.json();
  });
  const wheelName = `${pkg.name.replace(/-/g, "_")}-${pkg.version}-py3-none-any.whl`;
  const response = await fetch(new URL(wheelName, resolvedBaseUrl));
  if (!response.ok) throw new Error(`Unable to load ${wheelName}: HTTP ${response.status}`);
  const path = `/tmp/${wheelName}`;
  pyodide.FS.writeFile(path, new Uint8Array(await response.arrayBuffer()));
  pyodide.runPython(`import sys\nsys.path.insert(0, ${JSON.stringify(path)})`);
}

async function enterDemo(state) {
  if (state.activeDemo || state.stopped) return;
  const uploadConsent = document.getElementById("upload-consent")?.checked ?? false;

  window.history.pushState({ pydemo1: true }, "", `${window.location.pathname}${window.location.search}#pydemo1`);
  state.popstateHandler = () => exitDemo(state);
  window.addEventListener("popstate", state.popstateHandler, { once: true });

  const demo = createDemoView(CAPTURE_SECONDS, uploadConsent);
  state.activeDemo = demo;
  demo.exitButton.addEventListener("click", () => window.history.back());

  try {
    setLoadingMessage(demo, py ? "Local Python runtime ready" : "Preparing the local Python runtime...");
    await loadPythonRuntime();
    if (demo.stopped) return;
    await py.run(platformFor(state, demo));
  } catch (error) {
    demo.loadingScreen.remove();
    demo.speechStatus.textContent = String(error);
    // A failed demo never reaches the completion hand-off, so surface the exit control here instead.
    demo.timerChip.hidden = true;
    demo.exitButton.hidden = false;
    log(`demo failed: ${String(error)}`);
  }
}

function createDemoView(captureSeconds, uploadConsent) {
  const overlay = document.createElement("main");
  overlay.id = "pydemo1-view";
  overlay.setAttribute("aria-label", "Eye and speech detection demo");
  // Row sizing lives in a media-queried rule rather than the inline style: the speech waveform only needs a
  // slim strip, so wide (desktop/landscape) screens give its row to the eye panel; portrait phones keep more.
  const layoutStyle = document.createElement("style");
  const portraitRows = "#pydemo1-view{grid-template-rows:60dvh 25dvh 15dvh}";
  const wideRows = "@media (min-aspect-ratio:4/3){#pydemo1-view{grid-template-rows:72dvh 18dvh 10dvh}}";
  layoutStyle.textContent = portraitRows + wideRows;
  overlay.style.cssText = css([
    "display:grid",
    "width:100vw",
    "height:100dvh",
    "max-width:none",
    "margin:0",
    "padding:0",
    "overflow:hidden",
    "border:0",
    "background:#171b1f",
    "color:#effaf8",
    "box-shadow:none",
    "backdrop-filter:none",
    "font-family:ui-monospace,SFMono-Regular,Menlo,monospace",
  ]);

  const eyePanel = document.createElement("section");
  eyePanel.style.cssText = css([
    "position:relative",
    "min-height:0",
    "margin:6px 6px 3px",
    "overflow:hidden",
    "border:2px solid rgba(174,187,191,.48)",
    "border-radius:16px",
    "background:#03090e",
    "box-shadow:0 14px 34px rgba(0,0,0,.28),inset 0 1px 0 rgba(255,255,255,.035)",
  ]);
  const video = document.createElement("video");
  video.autoplay = true;
  video.muted = true;
  video.playsInline = true;
  video.style.cssText = css([
    "position:absolute",
    "inset:0",
    "display:block",
    "width:100%",
    "height:100%",
    "min-width:100%",
    "min-height:100%",
    "max-width:none",
    "max-height:none",
    "object-fit:cover",
    "object-position:50% 50%",
    "opacity:0",
    "pointer-events:none",
    "z-index:1",
  ]);
  const eyeCanvas = document.createElement("canvas");
  eyeCanvas.style.cssText = css([
    "position:absolute",
    "inset:0",
    "display:block",
    "width:100%",
    "height:100%",
    "min-width:100%",
    "min-height:100%",
    "max-width:none",
    "max-height:none",
    "margin:0",
    "object-fit:cover",
    "object-position:50% 50%",
    "background:transparent",
    "z-index:2",
  ]);
  const eyeBadge = createPanelBadge("EYE DETECTION");
  eyeBadge.style.top = "20px";
  eyePanel.append(video, eyeCanvas, eyeBadge);

  const speechPanel = document.createElement("section");
  speechPanel.style.cssText = css([
    "position:relative",
    "min-height:0",
    "margin:3px 6px",
    "border:2px solid rgba(174,187,191,.4)",
    "border-radius:14px",
    "background:#0d1a23",
    "overflow:hidden",
    "box-shadow:0 12px 28px rgba(0,0,0,.22),inset 0 1px 0 rgba(255,255,255,.03)",
  ]);
  const speechCanvas = document.createElement("canvas");
  speechCanvas.width = 1600;
  speechCanvas.height = 400;
  speechCanvas.style.cssText = css([
    "display:block",
    "width:100%",
    "height:100%",
    "max-width:none",
    "max-height:none",
    "margin:0",
    "background:#0d1a23",
  ]);
  const speechBadge = createPanelBadge("SPEECH DETECTION");
  const countdown = document.createElement("span");
  countdown.setAttribute("role", "timer");
  countdown.setAttribute("aria-live", "polite");
  countdown.textContent = `${captureSeconds} seconds remaining`;
  countdown.style.cssText = css([
    "position:absolute",
    "width:1px",
    "height:1px",
    "padding:0",
    "margin:-1px",
    "overflow:hidden",
    "clip:rect(0,0,0,0)",
    "white-space:nowrap",
    "border:0",
  ]);
  const speechStatus = document.createElement("span");
  speechStatus.textContent = "Preparing microphone...";
  speechStatus.style.cssText = "position:absolute;left:22px;bottom:16px;color:#adc8c9;font-size:15px";
  speechPanel.append(speechCanvas, speechBadge, countdown, speechStatus);

  const footer = document.createElement("footer");
  footer.setAttribute("aria-label", "GPU utilisation");
  footer.style.cssText = css(["position:relative", "min-height:0", "overflow:visible", "background:transparent"]);
  // Bottom-left slot: the countdown chip runs for the whole recording, then swaps for the Exit Demo button
  // once both detections complete.
  const controls = document.createElement("div");
  controls.style.cssText = css([
    "position:absolute",
    "left:6px",
    "bottom:6px",
    "z-index:2",
    "display:flex",
    "align-items:center",
    "gap:10px",
  ]);
  const timerChip = document.createElement("span");
  timerChip.setAttribute("aria-hidden", "true");
  timerChip.textContent = `${captureSeconds}s`;
  timerChip.style.cssText = css([
    "display:inline-flex",
    "align-items:center",
    "justify-content:center",
    "min-width:76px",
    "height:44px",
    "padding:0 16px",
    "border:1px solid rgba(174,187,191,.58)",
    "border-radius:8px",
    "background:rgba(10,26,37,.94)",
    "color:#edf4f3",
    "font:700 15px ui-monospace,monospace",
    "letter-spacing:.02em",
    "box-shadow:0 8px 22px rgba(0,0,0,.28)",
  ]);
  const exitButton = document.createElement("button");
  exitButton.type = "button";
  exitButton.hidden = true;
  exitButton.setAttribute("aria-label", "Exit demo");
  exitButton.style.cssText = css([
    "display:inline-flex",
    "align-items:center",
    "justify-content:center",
    "gap:9px",
    "height:44px",
    "padding:0 16px",
    "border:1px solid rgba(174,187,191,.58)",
    "border-radius:8px",
    "background:rgba(10,26,37,.94)",
    "color:#edf4f3",
    "font:700 13px ui-monospace,monospace",
    "letter-spacing:.02em",
    "cursor:pointer",
    "box-shadow:0 8px 22px rgba(0,0,0,.28)",
  ]);
  const exitIcon = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  exitIcon.setAttribute("viewBox", "0 0 24 24");
  exitIcon.setAttribute("aria-hidden", "true");
  exitIcon.style.cssText = "width:19px;height:19px;flex:none;fill:currentColor";
  const exitPath = document.createElementNS("http://www.w3.org/2000/svg", "path");
  exitPath.setAttribute("d", "M17 3h-6v2h6v14h-6v2h6a2 2 0 0 0 2-2V5a2 2 0 0 0-2-2ZM10 7l-5 5 5 5v-3h6v-4h-6V7Z");
  exitIcon.append(exitPath);
  const exitLabel = document.createElement("span");
  exitLabel.textContent = "Exit Demo";
  exitButton.append(exitIcon, exitLabel);
  controls.append(timerChip, exitButton);
  footer.append(controls);

  const loading = createLoadingScreen();
  overlay.append(layoutStyle, eyePanel, speechPanel, footer, loading.screen);
  const gpuMeterOverlay = document.getElementById("gpu-utilisation-meter");
  const previousGpuMeterStyle = gpuMeterOverlay?.getAttribute("style") ?? null;
  const previousBodyNodes = Array.from(document.body.childNodes);
  const previousBodyStyle = document.body.getAttribute("style");
  document.body.replaceChildren(overlay);
  if (gpuMeterOverlay) {
    footer.append(gpuMeterOverlay);
    Object.assign(gpuMeterOverlay.style, {
      position: "absolute",
      top: "auto",
      left: "auto",
      right: "6px",
      bottom: "6px",
      margin: "0",
      transform: "none",
      zIndex: "2",
    });
  }
  document.body.style.cssText = "display:block;margin:0;overflow:hidden;background:#171b1f";
  return {
    overlay,
    video,
    eyeCanvas,
    eyeFrame: null,
    speechCanvas,
    exitButton,
    timerChip,
    speechStatus,
    countdown,
    captureSeconds,
    uploadConsent,
    landmarker: null,
    speechSession: null,
    stream: null,
    audioContext: null,
    retryAudioStream: null,
    animationFrame: 0,
    videoFrameCallback: 0,
    stopped: false,
    keepMediaActive: false,
    peaks: [],
    level: 0,
    speechDetected: null,
    speechConfidence: 0,
    speechProbabilities: [],
    expectedPeakCount: 1,
    phase: "LOADING",
    speechRunning: false,
    speechStartedAt: 0,
    startedAt: 0,
    eyeResults: [],
    eyeAnalysis: null,
    eyeCrop: null,
    eyeComplete: false,
    loadingScreen: loading.screen,
    loadingMessage: loading.message,
    gpuMeterOverlay,
    previousGpuMeterStyle,
    previousBodyNodes,
    previousBodyStyle,
  };
}

function createLoadingScreen() {
  const screen = document.createElement("section");
  screen.setAttribute("role", "status");
  screen.setAttribute("aria-live", "polite");
  screen.style.cssText = css([
    "position:fixed",
    "inset:0",
    "z-index:20",
    "display:grid",
    "place-items:center",
    "background:#171b1f",
    "backdrop-filter:blur(12px)",
  ]);
  const card = document.createElement("div");
  card.style.cssText = css([
    "display:flex",
    "min-width:min(76vw,320px)",
    "align-items:center",
    "gap:18px",
    "padding:24px",
    "border:1px solid rgba(174,187,191,.46)",
    "border-radius:20px",
    "background:#0a1a25",
    "box-shadow:0 24px 80px rgba(0,0,0,.45)",
  ]);
  const spinner = document.createElement("span");
  spinner.setAttribute("aria-hidden", "true");
  spinner.style.cssText = css([
    "width:34px",
    "height:34px",
    "flex:none",
    "border:4px solid rgba(94,224,194,.18)",
    "border-top-color:#5ee0c2",
    "border-radius:50%",
    "animation:pydemo1-spin .8s linear infinite",
  ]);
  const copy = document.createElement("div");
  const title = document.createElement("strong");
  title.textContent = "Preparing local AI";
  title.style.cssText = "display:block;color:#effffb;font-size:17px";
  const message = document.createElement("span");
  message.textContent = "Starting the demo...";
  message.style.cssText = "display:block;margin-top:6px;color:#94bab8;font-size:13px";
  const style = document.createElement("style");
  style.textContent = "@keyframes pydemo1-spin{to{transform:rotate(360deg)}}";
  copy.append(title, message);
  card.append(spinner, copy);
  screen.append(card, style);
  return { screen, message };
}

function setLoadingMessage(demo, message) {
  demo.loadingMessage.textContent = message;
}

function createPanelBadge(text) {
  const badge = document.createElement("span");
  badge.textContent = text;
  badge.style.cssText = css([
    "position:absolute",
    "left:20px",
    "top:16px",
    "padding:8px 12px",
    "border:1px solid rgba(174,187,191,.48)",
    "border-radius:999px",
    "background:rgba(3,13,19,.72)",
    "color:#dffbf4",
    "font-size:12px",
    "font-weight:800",
    "letter-spacing:.08em",
    "backdrop-filter:blur(8px)",
    "z-index:3",
  ]);
  return badge;
}

async function connectClient(state) {
  const wasmAgent = await import("/modules/et-ws-wasm-agent/et_ws_wasm_agent.js");
  await wasmAgent.default();
  const protocol = window.location.protocol === "https:" ? "wss:" : "ws:";
  state.client = new wasmAgent.WsClient(new wasmAgent.WsClientConfig(`${protocol}//${window.location.host}/ws`));
  state.client.connect();
}

function platformFor(state, demo) {
  return {
    set_loading_message: (message) => setLoadingMessage(demo, message),
    connect_ws: () => connectClient(state),
    ws_state: () => state.client?.get_state() ?? "disconnected",
    agent_id: () => state.client?.get_agent_id(),
    load_models: () => loadModels(demo),
    show_demo: () => demo.loadingScreen.remove(),
    capture: () => startCapture(state, demo),
    should_stop: () => demo.stopped || runtime !== state,
    cleanup: () => {
      if (!demo.keepMediaActive) stopMedia(demo);
    },
    sleep,
    log,
  };
}

async function loadModels(demo) {
  setLoadingMessage(demo, "Loading eye and speech detection models...");
  [demo.landmarker, demo.speechSession] = await initializeModels(demo);
}

async function initializeModels(demo) {
  configureOnnxRuntime();
  let eyeReady = false;
  let speechReady = false;
  const reportModelProgress = () => {
    if (eyeReady && !speechReady) setLoadingMessage(demo, "Eye model ready; finishing the speech model...");
    if (speechReady && !eyeReady) setLoadingMessage(demo, "Speech model ready; finishing the eye model...");
  };
  const eyeModel = (async () => {
    const vision = await import(cfg.eye.bundle_path);
    const fileset = await vision.FilesetResolver.forVisionTasks(cfg.eye.wasm_path);
    const landmarker = await vision.FaceLandmarker.createFromOptions(fileset, {
      baseOptions: { modelAssetPath: cfg.eye.model_path, delegate: "GPU" },
      runningMode: "VIDEO",
      numFaces: 1,
    });
    eyeReady = true;
    reportModelProgress();
    return landmarker;
  })();
  const speechOptions = { executionProviders: ["wasm"] };
  const speechModelPromise = globalThis.ort.InferenceSession.create(cfg.speech.model_path, speechOptions);
  const speechModel = speechModelPromise.then((session) => {
    speechReady = true;
    reportModelProgress();
    return session;
  });
  return Promise.all([eyeModel, speechModel]);
}

async function startCapture(state, demo) {
  if (demo.stopped) return;
  demo.stream = await navigator.mediaDevices.getUserMedia({
    audio: true,
    video: { facingMode: "user" },
  });
  demo.video.srcObject = demo.stream;
  for (let index = 0; demo.video.videoWidth === 0 && index < 50; index++) await sleep(100);
  if (!demo.video.videoWidth) throw new Error("Camera metadata did not load");
  await demo.video.play();
  startVideoRendering(demo);

  demo.phase = "RECORDING";
  demo.startedAt = performance.now();
  demo.eyeComplete = false;
  py.reset_eye_capture();
  const speechPromise = runSpeechDetection(state, demo);
  const eyePromise = runEyeDetection(state, demo);
  await Promise.all([speechPromise, eyePromise]);
  if (!demo.stopped) {
    demo.keepMediaActive = true;
    demo.timerChip.hidden = true;
    demo.exitButton.hidden = false;
  }
}

async function runSpeechDetection(state, demo) {
  if (demo.stopped || demo.speechRunning) return;
  demo.speechRunning = true;
  demo.phase = "RECORDING";
  demo.speechStartedAt = performance.now();
  demo.peaks = [];
  demo.level = 0;
  demo.speechDetected = null;
  demo.speechConfidence = 0;
  demo.speechProbabilities = [];
  demo.speechStatus.hidden = false;
  demo.speechStatus.textContent = "Listening for speech";
  demo.countdown.textContent = `${demo.captureSeconds} seconds remaining`;
  drawSpeechPanel(demo);
  const audio = await recordAudio(demo);
  if (demo.stopped) return;
  const result = py
    .process_speech_capture(pyodide.toPy(audio.probabilities), audio.sourceRate, audio.recordedSeconds)
    .toJs({ dict_converter: Object.fromEntries });
  demo.speechDetected = result.speech_detected;
  demo.speechConfidence = result.confidence;
  demo.phase = "COMPLETE";
  demo.countdown.textContent = "Complete";
  demo.speechStatus.textContent = result.speech_detected
    ? `Speech detected - ${(result.confidence * 100).toFixed(1)}% peak confidence`
    : "No speech detected";
  demo.speechStatus.hidden = result.speech_detected;
  state.client?.send?.(result.event_json);
  drawSpeechPanel(demo);
  demo.speechRunning = false;
}

async function runEyeDetection(state, demo) {
  let lastInference = 0;
  while (!demo.stopped && performance.now() - demo.startedAt < demo.captureSeconds * 1000) {
    const now = performance.now();

    if (now - lastInference >= 50) {
      lastInference = now;
      captureEyeFrame(demo);
      const result = demo.landmarker.detectForVideo(demo.eyeFrame, now);
      const faces = (result.faceLandmarks ?? []).map((face) => {
        const flat = [];
        for (const point of face) flat.push(point.x, point.y);
        return flat;
      });
      const capture = JSON.stringify({
        faces,
        width: demo.video.videoWidth,
        height: demo.video.videoHeight,
        upload_consent: demo.uploadConsent,
      });
      const processed = py.process_eye_capture(capture).toJs({ dict_converter: Object.fromEntries });
      const eyePayload = JSON.parse(processed.results_json);
      demo.eyeResults = eyePayload.faces;
      demo.eyeAnalysis = eyePayload.analysis;
      demo.eyeCrop = eyePayload.crop;
      if (processed.event_json) state.client?.send?.(processed.event_json);
      for (let index = 0; index < processed.capture_count; index++) {
        await saveEyeCapture(state, demo);
      }
    }
    await nextFrame();
  }
  if (!demo.stopped) {
    demo.eyeComplete = true;
    renderEyeFrame(demo);
    stopVideo(demo);
  }
}

async function saveEyeCapture(state, demo) {
  try {
    const blob = await new Promise((resolve, reject) => {
      demo.eyeCanvas.toBlob(
        (result) => (result ? resolve(result) : reject(new Error("canvas.toBlob returned null"))),
        "image/png",
      );
    });
    const bytes = new Uint8Array(await blob.arrayBuffer());
    const agentId = state.client.get_agent_id();
    const filename = `pydemo1-eye-capture-${Date.now()}.png`;
    const response = await fetch(`/storage/${agentId}/${filename}`, { method: "PUT", body: bytes });
    if (!response.ok) {
      throw new Error(`eye capture upload failed: ${response.status} ${response.statusText}`);
    }
    state.client?.send?.(py.eye_capture_stored_json(agentId, filename));
    log(`eye capture saved to storage and broadcast: ${filename} (${bytes.length} bytes)`);
  } catch (error) {
    const message = String(error);
    log(`eye capture failed: ${message}`);
    state.client?.send?.(py.eye_capture_error_json(message));
  }
}

function renderEyeFrame(demo) {
  if (!demo.video.videoWidth) return;
  const canvas = demo.eyeCanvas;
  const ctx = canvas.getContext("2d");
  const sourceWidth = demo.video.videoWidth;
  const sourceHeight = demo.video.videoHeight;
  const displayWidth = Math.max(canvas.clientWidth, 1);
  const displayHeight = Math.max(canvas.clientHeight, 1);
  const displayAspect = displayWidth / displayHeight;
  const renderWidth = Math.min(Math.round(displayWidth * Math.min(window.devicePixelRatio || 1, 2)), 720);
  const renderHeight = Math.max(1, Math.round(renderWidth / displayAspect));
  if (canvas.width !== renderWidth || canvas.height !== renderHeight) {
    canvas.width = renderWidth;
    canvas.height = renderHeight;
  }
  const [cropLeft, cropTop, cropRight, cropBottom] = demo.eyeCrop ?? [0, 0, sourceWidth, sourceHeight];
  const cropWidth = Math.max(cropRight - cropLeft, 1);
  const cropHeight = Math.max(cropBottom - cropTop, 1);
  const scale = Math.min(canvas.width / cropWidth, canvas.height / cropHeight);
  const renderedWidth = cropWidth * scale;
  const renderedHeight = cropHeight * scale;
  const offsetX = (canvas.width - renderedWidth) * 0.5;
  const offsetY = (canvas.height - renderedHeight) * 0.5;
  ctx.fillStyle = "#03090e";
  ctx.fillRect(0, 0, canvas.width, canvas.height);
  ctx.drawImage(demo.video, cropLeft, cropTop, cropWidth, cropHeight, offsetX, offsetY, renderedWidth, renderedHeight);
  const renderX = (value) => offsetX + (value - cropLeft) * scale;
  const renderY = (value) => offsetY + (value - cropTop) * scale;
  ctx.font = "700 18px ui-monospace,monospace";
  for (const result of demo.eyeResults) {
    ctx.lineWidth = 2;
    ctx.strokeStyle = "rgba(202,225,229,.45)";
    const [faceLeft, faceTop, faceRight, faceBottom] = result.face_box;
    ctx.strokeRect(renderX(faceLeft), renderY(faceTop), (faceRight - faceLeft) * scale, (faceBottom - faceTop) * scale);
    for (const eye of result.eyes) {
      const [left, top, right, bottom] = eye.box;
      const renderedLeft = renderX(left);
      const renderedTop = renderY(top);
      const color = EYE_COLORS[eye.label] ?? "#ffffff";
      ctx.lineWidth = 5;
      ctx.strokeStyle = color;
      ctx.shadowColor = color;
      ctx.shadowBlur = 14;
      ctx.strokeRect(renderedLeft, renderedTop, (right - left) * scale, (bottom - top) * scale);
      ctx.shadowBlur = 0;
      drawOutlinedText(
        ctx,
        eye.label === "left_eye" ? "LEFT" : "RIGHT",
        renderedLeft,
        Math.max(renderedTop - 8, 20),
        color,
        4,
      );
    }
    for (const iris of result.irises ?? []) {
      const color = EYE_COLORS[iris.label] ?? "#ffffff";
      ctx.beginPath();
      ctx.strokeStyle = color;
      ctx.lineWidth = 3;
      ctx.arc(renderX(iris.center[0]), renderY(iris.center[1]), iris.radius * scale, 0, Math.PI * 2);
      ctx.stroke();
    }
  }
  renderEyeAnalysis(ctx, demo.eyeAnalysis);
  if (demo.eyeComplete) renderEyeComplete(ctx);
}

function renderEyeComplete(ctx) {
  const width = Math.min(ctx.canvas.width - 40, 520);
  const height = 78;
  const x = (ctx.canvas.width - width) * 0.5;
  const y = (ctx.canvas.height - height) * 0.5;
  ctx.save();
  ctx.fillStyle = "rgba(25,38,44,.92)";
  roundedRect(ctx, x, y, width, height, 14);
  ctx.strokeStyle = "rgba(190,201,203,.7)";
  ctx.lineWidth = 2;
  ctx.beginPath();
  ctx.roundRect(x, y, width, height, 14);
  ctx.stroke();
  ctx.fillStyle = "#e9efee";
  ctx.font = `800 ${Math.max(20, Math.min(28, width / 18))}px ui-monospace,monospace`;
  ctx.textAlign = "center";
  ctx.textBaseline = "middle";
  ctx.fillText("EYE DETECTION COMPLETE", ctx.canvas.width * 0.5, y + height * 0.5);
  ctx.restore();
}

function renderEyeAnalysis(ctx, analysis) {
  const screenings = [
    ["eye misalignment", analysis?.misalignment],
    ["rhythmic oscillation", analysis?.oscillation],
  ];
  const lines = [];
  for (const [name, metrics] of screenings) {
    if (metrics?.status === "ok") lines.push(`${name}: ${metrics.detected ? "DETECTED" : "none"}`);
  }
  if (!lines.length) return;
  lines.push("screening demo -- not a medical diagnosis");

  const fontSize = Math.max(22, Math.min(28, Math.round(ctx.canvas.width / 26)));
  const lineHeight = Math.round(fontSize * 1.35);
  const paddingX = Math.round(fontSize * 0.35);
  const paddingY = Math.round(fontSize * 0.18);
  const margin = Math.round(fontSize * 0.55);
  ctx.save();
  ctx.font = `600 ${fontSize}px ui-monospace, monospace`;
  ctx.textBaseline = "top";
  const firstLineY = ctx.canvas.height - margin - lines.length * lineHeight;
  for (const [index, textLine] of lines.entries()) {
    const lineY = firstLineY + index * lineHeight;
    const backgroundWidth = Math.min(ctx.canvas.width - margin * 2, ctx.measureText(textLine).width + paddingX * 2);
    ctx.fillStyle = "rgba(5, 12, 18, 0.78)";
    ctx.fillRect(margin, lineY - paddingY, backgroundWidth, fontSize + paddingY * 2);
    const color = textLine.includes("DETECTED") ? "#1e90ff" : "#d7e0e8";
    drawOutlinedText(ctx, textLine, margin + paddingX, lineY, color);
  }
  ctx.restore();
}

function drawOutlinedText(ctx, text, x, y, fillColor, lineWidth = 3) {
  ctx.lineJoin = "round";
  ctx.lineWidth = lineWidth;
  ctx.strokeStyle = "rgba(0, 0, 0, 0.85)";
  ctx.strokeText(text, x, y);
  ctx.fillStyle = fillColor;
  ctx.fillText(text, x, y);
}

function startVideoRendering(demo) {
  const render = () => {
    if (demo.stopped) return;
    renderEyeFrame(demo);
    if (demo.video.requestVideoFrameCallback) {
      demo.videoFrameCallback = demo.video.requestVideoFrameCallback(render);
    } else {
      demo.animationFrame = requestAnimationFrame(render);
    }
  };
  render();
}

function captureEyeFrame(demo) {
  const frame = demo.eyeFrame ?? document.createElement("canvas");
  demo.eyeFrame = frame;
  const inferenceWidth = Math.min(demo.video.videoWidth, 480);
  const inferenceHeight = Math.round((inferenceWidth * demo.video.videoHeight) / demo.video.videoWidth);
  if (frame.width !== inferenceWidth || frame.height !== inferenceHeight) {
    frame.width = inferenceWidth;
    frame.height = inferenceHeight;
  }
  frame.getContext("2d").drawImage(demo.video, 0, 0, frame.width, frame.height);
}

async function recordAudio(demo) {
  const AudioContext = globalThis.AudioContext ?? globalThis.webkitAudioContext;
  if (!AudioContext) throw new Error("Web Audio is unavailable");
  demo.audioContext = new AudioContext();
  await demo.audioContext.resume();
  const sourceRate = demo.audioContext.sampleRate;
  demo.expectedPeakCount = Math.ceil((demo.captureSeconds * sourceRate) / 4096) * 8;
  const audioStream = await activeAudioStream(demo);
  const source = demo.audioContext.createMediaStreamSource(audioStream);
  const processor = demo.audioContext.createScriptProcessor(4096, 1, 1);
  const sink = demo.audioContext.createGain();
  sink.gain.value = 0;
  const analyzer = createSpeechAnalyzer(demo.speechSession, sourceRate, demo.speechProbabilities);
  let analysisPromise = Promise.resolve();
  processor.onaudioprocess = (event) => {
    const block = new Float32Array(event.inputBuffer.getChannelData(0));
    addSpeechSamples(demo, block);
    drawSpeechPanel(demo);
    analysisPromise = analysisPromise.then(() => analyzeSpeechBlock(analyzer, block));
  };
  source.connect(processor);
  processor.connect(sink);
  sink.connect(demo.audioContext.destination);
  await sleep(demo.captureSeconds * 1000);
  processor.disconnect();
  source.disconnect();
  sink.disconnect();
  const recordedSeconds = (performance.now() - demo.speechStartedAt) / 1000;
  await analysisPromise;
  await flushSpeechAnalyzer(analyzer);
  await demo.audioContext.close();
  demo.audioContext = null;
  if (audioStream === demo.retryAudioStream) {
    for (const track of audioStream.getTracks()) track.stop();
    demo.retryAudioStream = null;
  }
  return {
    probabilities: analyzer.probabilities,
    sourceRate,
    recordedSeconds,
  };
}

async function activeAudioStream(demo) {
  const liveTrack = demo.stream?.getAudioTracks?.().find((track) => track.readyState === "live");
  if (liveTrack) return demo.stream;
  demo.retryAudioStream = await navigator.mediaDevices.getUserMedia({ audio: true, video: false });
  return demo.retryAudioStream;
}

function addSpeechSamples(demo, samples) {
  let energy = 0;
  for (const sample of samples) energy += sample * sample;
  const rms = Math.sqrt(energy / Math.max(samples.length, 1));
  demo.level += (rms - demo.level) * 0.35;
  const bins = 8;
  const binSize = Math.max(1, Math.floor(samples.length / bins));
  for (let bin = 0; bin < bins; bin++) {
    let low = 0;
    let high = 0;
    const end = Math.min((bin + 1) * binSize, samples.length);
    for (let index = bin * binSize; index < end; index++) {
      low = Math.min(low, samples[index]);
      high = Math.max(high, samples[index]);
    }
    demo.peaks.push({ low, high });
  }
}

function drawSpeechPanel(demo) {
  const canvas = demo.speechCanvas;
  resizeCanvasToDisplaySize(canvas);
  const ctx = canvas.getContext("2d");
  const width = canvas.width;
  const height = canvas.height;
  const elapsed = demo.speechStartedAt
    ? Math.min((performance.now() - demo.speechStartedAt) / 1000, demo.captureSeconds)
    : 0;
  const progress = demo.phase === "LOADING" ? 0 : Math.min(elapsed / demo.captureSeconds, 1);
  const remaining = Math.max(Math.ceil(demo.captureSeconds - elapsed), 0);
  if (demo.phase === "RECORDING") {
    demo.countdown.textContent = `${remaining} seconds remaining`;
    demo.timerChip.textContent = `${remaining}s`;
  }
  const background = ctx.createLinearGradient(0, 0, width, height);
  background.addColorStop(0, "#0c1924");
  background.addColorStop(0.55, "#18313a");
  background.addColorStop(1, "#0c1b23");
  ctx.fillStyle = background;
  ctx.fillRect(0, 0, width, height);

  ctx.strokeStyle = "rgba(157,203,205,.07)";
  ctx.lineWidth = 1;
  for (let x = 50; x < width; x += 80) line(ctx, x, 70, x, height - 48);
  for (let y = 100; y < height - 40; y += 55) line(ctx, 40, y, width - 40, y);

  const center = height * 0.55;
  if (demo.peaks.length) {
    const columns = [];
    const maximumColumns = Math.max(1, Math.min(480, Math.floor((width - 80) / 2)));
    const peaksPerColumn = Math.max(1, Math.ceil(demo.expectedPeakCount / maximumColumns));
    const expectedProbabilityCount = Math.ceil((demo.captureSeconds * cfg.speech.sample_rate) / cfg.speech.chunk_size);
    for (let peakStart = 0; peakStart < demo.peaks.length; peakStart += peaksPerColumn) {
      const end = Math.min(peakStart + peaksPerColumn, demo.peaks.length);
      let low = 0;
      let high = 0;
      for (let index = peakStart; index < end; index++) {
        low = Math.min(low, demo.peaks[index].low);
        high = Math.max(high, demo.peaks[index].high);
      }
      const progressIndex = peakStart / Math.max(demo.expectedPeakCount - 1, 1);
      const probabilityIndex = Math.min(
        Math.floor(progressIndex * expectedProbabilityCount),
        expectedProbabilityCount - 1,
      );
      columns.push({
        high,
        low,
        speech: (demo.speechProbabilities[probabilityIndex] ?? 0) >= cfg.speech.threshold,
        x: 40 + progressIndex * (width - 80),
      });
    }

    ctx.strokeStyle = "rgba(190,199,201,.72)";
    ctx.lineWidth = 2;
    ctx.beginPath();
    for (const column of columns) {
      ctx.moveTo(column.x, center + column.low * 135);
      ctx.lineTo(column.x, center + column.high * 135);
    }
    ctx.stroke();

    const gradient = ctx.createLinearGradient(40, 0, width - 40, 0);
    gradient.addColorStop(0, "#659ac4");
    gradient.addColorStop(0.5, "#65b7a5");
    gradient.addColorStop(1, "#a5bd7c");
    ctx.strokeStyle = gradient;
    ctx.lineWidth = 2;
    ctx.shadowColor = "rgba(83,175,154,.3)";
    ctx.shadowBlur = 10;
    ctx.beginPath();
    for (const column of columns) {
      if (!column.speech) continue;
      ctx.moveTo(column.x, center + column.low * 135);
      ctx.lineTo(column.x, center + column.high * 135);
    }
    ctx.stroke();
    ctx.shadowBlur = 0;
  }

  ctx.fillStyle = "rgba(255,255,255,.1)";
  ctx.fillRect(40, height - 24, width - 80, 6);
  ctx.fillStyle = "#67b7a3";
  ctx.fillRect(40, height - 24, (width - 80) * progress, 6);

  if (demo.speechDetected === true) {
    const badgeWidth = 620;
    const badgeHeight = 106;
    const x = (width - badgeWidth) / 2;
    // The slimmer desktop speech row can leave less canvas than the badge's usual bottom offset needs.
    const y = Math.max(height - 148, 6);
    ctx.fillStyle = "rgba(28,73,66,.95)";
    roundedRect(ctx, x, y, badgeWidth, badgeHeight, 20);
    ctx.fillStyle = "#eafffa";
    ctx.font = "800 44px ui-monospace,monospace";
    ctx.fillText("SPEECH DETECTED", x + 38, y + 57);
    ctx.fillStyle = "#acd1c7";
    ctx.font = "700 25px ui-monospace,monospace";
    ctx.fillText(`${(demo.speechConfidence * 100).toFixed(1)}% peak confidence`, x + 40, y + 90);
  }
}

function createSpeechAnalyzer(session, sourceRate, probabilities) {
  return {
    session,
    sourceRate,
    recurrentState: new Float32Array(2 * 128),
    context: new Float32Array(cfg.speech.context_size),
    pending: new Float32Array(),
    probabilities,
  };
}

async function analyzeSpeechBlock(analyzer, sourceSamples) {
  const samples = resample(sourceSamples, analyzer.sourceRate, cfg.speech.sample_rate);
  analyzer.pending = concatenate([analyzer.pending, samples]);
  while (analyzer.pending.length >= cfg.speech.chunk_size) {
    await inferSpeechChunk(analyzer, analyzer.pending.subarray(0, cfg.speech.chunk_size));
    analyzer.pending = analyzer.pending.slice(cfg.speech.chunk_size);
  }
}

async function flushSpeechAnalyzer(analyzer) {
  if (analyzer.pending.length) {
    await inferSpeechChunk(analyzer, analyzer.pending);
    analyzer.pending = new Float32Array();
  }
}

async function inferSpeechChunk(analyzer, samples) {
  const input = new Float32Array(cfg.speech.context_size + cfg.speech.chunk_size);
  input.set(analyzer.context);
  input.set(samples, cfg.speech.context_size);
  const outputs = await analyzer.session.run({
    input: new globalThis.ort.Tensor("float32", input, [1, input.length]),
    state: new globalThis.ort.Tensor("float32", analyzer.recurrentState, [2, 1, 128]),
    sr: new globalThis.ort.Tensor("int64", BigInt64Array.of(BigInt(cfg.speech.sample_rate)), []),
  });
  analyzer.probabilities.push(Number(outputs.output.data[0]));
  analyzer.recurrentState = new Float32Array(outputs.stateN.data);
  analyzer.context = input.slice(input.length - cfg.speech.context_size);
}

function configureOnnxRuntime() {
  const wasm = globalThis.ort?.env?.wasm;
  if (!wasm) throw new Error("onnxruntime-web is unavailable");
  const base = "/modules/onnxruntime-web/dist";
  wasm.numThreads = globalThis.crossOriginIsolated && globalThis.SharedArrayBuffer ? 0 : 1;
  wasm.wasmPaths = {
    mjs: `${base}/ort-wasm-simd-threaded.mjs`,
    wasm: `${base}/ort-wasm-simd-threaded.wasm`,
  };
}

function exitDemo(state) {
  const demo = state.activeDemo;
  if (demo) {
    demo.stopped = true;
    stopMedia(demo);
    demo.landmarker?.close?.();
    demo.speechSession?.release?.();
    const restoredNodes = demo.previousBodyNodes.filter((node) => {
      if (node !== demo.gpuMeterOverlay) return true;
      return Boolean(demo.gpuMeterOverlay?.isConnected);
    });
    document.body.replaceChildren(...restoredNodes);
    if (demo.gpuMeterOverlay?.isConnected) {
      if (demo.previousGpuMeterStyle === null) {
        demo.gpuMeterOverlay.removeAttribute("style");
      } else {
        demo.gpuMeterOverlay.setAttribute("style", demo.previousGpuMeterStyle);
      }
    }
    if (demo.previousBodyStyle === null) {
      document.body.removeAttribute("style");
    } else {
      document.body.setAttribute("style", demo.previousBodyStyle);
    }
  }
  state.activeDemo = null;
  state.client?.disconnect?.();
  state.client = null;
  if (state.popstateHandler) window.removeEventListener("popstate", state.popstateHandler);
  state.popstateHandler = null;
  // Leaving the demo IS stopping the module now that run() launches straight into it, so the next Run press
  // relaunches immediately instead of toggling a stop first.
  if (runtime === state) runtime = null;
}

function stopMedia(demo) {
  stopVideo(demo);
  for (const track of demo.stream?.getTracks?.() ?? []) track.stop();
  demo.stream = null;
  for (const track of demo.retryAudioStream?.getTracks?.() ?? []) track.stop();
  demo.retryAudioStream = null;
  demo.audioContext?.close?.();
  demo.audioContext = null;
}

function stopVideo(demo) {
  if (demo.videoFrameCallback && demo.video.cancelVideoFrameCallback) {
    demo.video.cancelVideoFrameCallback(demo.videoFrameCallback);
  }
  if (demo.animationFrame) cancelAnimationFrame(demo.animationFrame);
  demo.videoFrameCallback = 0;
  demo.animationFrame = 0;
  for (const track of demo.stream?.getVideoTracks?.() ?? []) track.stop();
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
  for (let index = 0; index < output.length; index++) {
    const position = index * ratio;
    const left = Math.min(Math.floor(position), input.length - 1);
    const right = Math.min(left + 1, input.length - 1);
    const fraction = position - left;
    output[index] = input[left] * (1 - fraction) + input[right] * fraction;
  }
  return output;
}

function resizeCanvasToDisplaySize(canvas) {
  const ratio = Math.min(window.devicePixelRatio || 1, 2);
  const width = Math.max(1, Math.round(canvas.clientWidth * ratio));
  const height = Math.max(1, Math.round(canvas.clientHeight * ratio));
  if (canvas.width !== width || canvas.height !== height) {
    canvas.width = width;
    canvas.height = height;
  }
}

function roundedRect(ctx, x, y, width, height, radius) {
  ctx.beginPath();
  ctx.roundRect(x, y, width, height, radius);
  ctx.fill();
}

function line(ctx, x1, y1, x2, y2) {
  ctx.beginPath();
  ctx.moveTo(x1, y1);
  ctx.lineTo(x2, y2);
  ctx.stroke();
}

function nextFrame() {
  return new Promise((resolve) => requestAnimationFrame(resolve));
}

function sleep(ms) {
  return new Promise((resolve) => window.setTimeout(resolve, ms));
}

function css(declarations) {
  return declarations.join(";");
}

function setMainStatus(message) {
  const output = document.getElementById("module-output");
  if (output) output.value = message;
}

function setPreparationStatus(message) {
  if (!runtime || runtime.stopped) return;
  if (runtime.activeDemo) {
    setLoadingMessage(runtime.activeDemo, message);
  } else {
    setMainStatus(`Preparing local AI: ${message}`);
  }
}

function log(message) {
  const logLine = `[pydemo1] ${message}`;
  console.log(logLine);
  const output = document.getElementById("log");
  if (output) output.textContent = output.textContent ? `${output.textContent}\n${logLine}` : logLine;
}
