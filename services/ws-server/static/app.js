import init, {
  BluetoothAccess,
  GeolocationReading,
  GpuInfo,
  GraphicsSupport,
  initTracing,
  MicrophoneAccess,
  NfcScanResult,
  SpeechRecognitionSession,
  VideoCapture,
  WebGpuProbeResult,
  WsClient,
  WsClientConfig,
} from "/pkg/et_ws_wasm_agent.js";

console.log("app.js: module loading started");

const logEl = document.getElementById("log");
const moduleSelect = document.getElementById("module-select");
const runModuleButton = document.getElementById("run-module-button");
const micButton = document.getElementById("mic-button");
const videoButton = document.getElementById("video-button");
const bluetoothButton = document.getElementById("bluetooth-button");
const geolocationButton = document.getElementById("geolocation-button");
const graphicsButton = document.getElementById("graphics-button");
const webgpuTestButton = document.getElementById("webgpu-test-button");
const gpuInfoButton = document.getElementById("gpu-info-button");
const speechButton = document.getElementById("speech-button");
const nfcButton = document.getElementById("nfc-button");
const sensorsButton = document.getElementById("sensors-button");
const videoOutputButton = document.getElementById("video-output-button");
const agentStatusEl = document.getElementById("agent-status");
const agentIdEl = document.getElementById("agent-id");
const sensorOutputEl = document.getElementById("sensor-output");
const videoOutputEl = document.getElementById("ml-debug-output");
const videoPreview = document.getElementById("video-preview");
const videoOutputCanvas = document.getElementById("video-output-canvas");

let microphone = null;
let videoCapture = null;
let bluetoothDevice = null;
let speechSession = null;
let speechListening = false;
let sensorsActive = false;
let orientationState = null;
let motionState = null;
let videoCvSession = null;
let videoCvInputName = null;
let videoCvOutputName = null;
let videoCvLoopId = null;
let videoCvInferencePending = false;
let lastVideoInferenceAt = 0;
let lastVideoCvLabel = null;
let videoCvCanvas = null;
let videoCvContext = null;
let videoOverlayContext = videoOutputCanvas.getContext("2d");
let videoOutputVisible = false;
let videoRenderFrameId = null;
let lastVideoInferenceSummary = null;
const loadedWorkflowModules = new Map();
let sendClientEvent = () => {};
const VIDEO_INFERENCE_INTERVAL_MS = 750;
const VIDEO_RENDER_SCORE_THRESHOLD = 0.35;
const VIDEO_MODEL_PATH = "/static/models/video_cv.onnx";
const VIDEO_FALLBACK_INPUT_SIZE = 224;
const STORED_AGENT_ID_KEY = "ws_wasm_agent.agent_id";
let currentAgentId = null;

const append = (line) => {
  logEl.textContent += `\n${line}`;
};

const describeError = (error) => (
  error instanceof Error ? error.message : String(error)
);

const WORKFLOW_MODULES = new Map();

const populateModuleDropdown = async () => {
  append("Discovering modules via /api/modules...");
  const resp = await fetch("/api/modules");
  if (!resp.ok) {
    append(`Failed to fetch module list from server: ${resp.status} ${resp.statusText}`);
    return;
  }
  const moduleNames = await resp.json();
  append(`Found ${moduleNames.length} potential modules: ${moduleNames.join(", ")}`);

  // Clear current options
  moduleSelect.innerHTML = "";

  for (const name of moduleNames) {
    try {
      const moduleKey = name;
      const moduleUrl = `/modules/${name}/pkg/et_ws_${name.replace(/-/g, "_")}.js`;
      const wasmUrl = `/modules/${name}/pkg/et_ws_${name.replace(/-/g, "_")}_bg.wasm`;

      append(`Loading metadata for ${name}...`);
      const loadedModule = await import(`${moduleUrl}?v=${Date.now()}`);
      await loadedModule.default(wasmUrl);

      let metadata = { name, description: "", version: "" };
      if (typeof loadedModule.metadata === "function") {
        metadata = loadedModule.metadata();
      }

      WORKFLOW_MODULES.set(moduleKey, {
        label: metadata.description || metadata.name || name,
        moduleUrl,
        wasmUrl,
        loaded: loadedModule,
      });

      const option = document.createElement("option");
      option.value = moduleKey;
      option.textContent = WORKFLOW_MODULES.get(moduleKey).label;
      moduleSelect.appendChild(option);

      append(`Successfully discovered module: ${name} (${metadata.version})`);
    } catch (error) {
      append(`Error discovering module ${name}: ${describeError(error)}`);
      console.error(`discovery error for ${name}:`, error);
    }
  }
};

const updateAgentCard = (status, agentId = currentAgentId) => {
  currentAgentId = agentId || null;
  agentStatusEl.textContent = status;
  agentIdEl.textContent = currentAgentId ?? "unassigned";
};

const readStoredAgentId = () => {
  try {
    return window.localStorage.getItem(STORED_AGENT_ID_KEY);
  } catch (error) {
    append(`agent storage read error: ${error instanceof Error ? error.message : String(error)}`);
    return null;
  }
};

const writeStoredAgentId = (agentId) => {
  try {
    window.localStorage.setItem(STORED_AGENT_ID_KEY, agentId);
  } catch (error) {
    append(`agent storage write error: ${error instanceof Error ? error.message : String(error)}`);
  }
};

const loadWorkflowModule = async (moduleKey) => {
  const moduleConfig = WORKFLOW_MODULES.get(moduleKey);
  if (!moduleConfig) {
    throw new Error(`unknown workflow module: ${moduleKey}`);
  }

  if (moduleConfig.loaded) {
    return moduleConfig.loaded;
  }

  // This part is mostly handled by populateModuleDropdown now
  // but kept for robustness if called separately.
  const cacheBust = Date.now();
  const moduleUrl = `${moduleConfig.moduleUrl}?v=${cacheBust}`;
  const wasmUrl = `${moduleConfig.wasmUrl}?v=${cacheBust}`;
  append(`${moduleConfig.label} module: importing ${moduleUrl}`);
  const loadedModule = await import(moduleUrl);
  append(`${moduleConfig.label} module: initializing ${wasmUrl}`);
  await loadedModule.default(wasmUrl);
  moduleConfig.loaded = loadedModule;
  return loadedModule;
};

const runSelectedWorkflowModule = async () => {
  const moduleKey = moduleSelect.value;
  const moduleConfig = WORKFLOW_MODULES.get(moduleKey);
  if (!moduleConfig) {
    throw new Error(`unknown workflow module: ${moduleKey}`);
  }

  const loadedModule = await loadWorkflowModule(moduleKey);
  append(`${moduleConfig.label} module: calling run()`);
  const runPromise = loadedModule.run();
  append(`${moduleConfig.label} module: run() started`);
  await runPromise;
  append(`${moduleConfig.label} module completed`);
};

const handleProtocolMessage = (message) => {
  let parsed;

  try {
    parsed = JSON.parse(message);
  } catch {
    return;
  }

  if (parsed?.type !== "connect_ack" || typeof parsed.agent_id !== "string") {
    return;
  }

  writeStoredAgentId(parsed.agent_id);

  if (parsed.status === "reconnected") {
    updateAgentCard("Reconnected with previously issued server ID.", parsed.agent_id);
    append(`agent_id reused: ${parsed.agent_id}`);
    return;
  }

  updateAgentCard("Server assigned a new agent ID.", parsed.agent_id);
  append(`agent_id assigned: ${parsed.agent_id}`);
};

const formatNumber = (value, digits = 3) => (
  Number.isFinite(value) ? value.toFixed(digits) : "n/a"
);

const renderSensorOutput = () => {
  const lines = [
    "Device sensor stream",
    `updated: ${new Date().toLocaleTimeString()}`,
    "",
    "orientation",
  ];

  if (orientationState) {
    lines.push(`alpha: ${formatNumber(orientationState.alpha)}`);
    lines.push(`beta: ${formatNumber(orientationState.beta)}`);
    lines.push(`gamma: ${formatNumber(orientationState.gamma)}`);
    lines.push(`absolute: ${orientationState.absolute === null ? "n/a" : String(orientationState.absolute)}`);
  } else {
    lines.push("waiting for orientation event...");
  }

  lines.push("");
  lines.push("motion");
  if (motionState) {
    lines.push(
      `acceleration: x=${formatNumber(motionState.acceleration?.x)} y=${formatNumber(motionState.acceleration?.y)} z=${
        formatNumber(motionState.acceleration?.z)
      }`,
    );
    lines.push(
      `acceleration including gravity: x=${formatNumber(motionState.accelerationIncludingGravity?.x)} y=${
        formatNumber(motionState.accelerationIncludingGravity?.y)
      } z=${formatNumber(motionState.accelerationIncludingGravity?.z)}`,
    );
    lines.push(
      `rotation rate: alpha=${formatNumber(motionState.rotationRate?.alpha)} beta=${
        formatNumber(motionState.rotationRate?.beta)
      } gamma=${formatNumber(motionState.rotationRate?.gamma)}`,
    );
    lines.push(`interval: ${formatNumber(motionState.interval, 1)} ms`);
  } else {
    lines.push("waiting for motion event...");
  }

  sensorOutputEl.value = lines.join("\n");
};

const setVideoOutput = (lines) => {
  videoOutputEl.value = Array.isArray(lines) ? lines.join("\n") : String(lines);
};

const updateVideoStatus = (extraLines = []) => {
  const inputMetadata = videoCvInputName
    ? videoCvSession?.inputMetadata?.[videoCvInputName]
    : null;
  const outputMetadata = videoCvOutputName
    ? videoCvSession?.outputMetadata?.[videoCvOutputName]
    : null;
  const lines = [
    `model: ${videoCvSession ? "loaded" : "not loaded"}`,
    `video: ${videoCapture ? "active" : "inactive"}`,
    `input: ${videoCvInputName ?? "n/a"}`,
    `output: ${videoCvOutputName ?? "n/a"}`,
    `input dims: ${JSON.stringify(inputMetadata?.dimensions ?? [])}`,
    `output dims: ${JSON.stringify(outputMetadata?.dimensions ?? [])}`,
    `loop: ${videoCvLoopId === null ? "idle" : "running"}`,
    `display: ${videoOutputVisible ? "visible" : "hidden"}`,
    `mode: ${lastVideoInferenceSummary?.mode ?? "unknown"}`,
  ];
  setVideoOutput(lines.concat("", extraLines));
};

const handleOrientation = (event) => {
  orientationState = {
    alpha: event.alpha,
    beta: event.beta,
    gamma: event.gamma,
    absolute: typeof event.absolute === "boolean" ? event.absolute : null,
  };
  renderSensorOutput();
};

const handleMotion = (event) => {
  const accelerationIncludingGravity = event.accelerationIncludingGravity
    ? {
      x: event.accelerationIncludingGravity.x ?? 0,
      y: event.accelerationIncludingGravity.y ?? 0,
      z: event.accelerationIncludingGravity.z ?? 0,
    }
    : null;

  motionState = {
    acceleration: event.acceleration
      ? {
        x: event.acceleration.x,
        y: event.acceleration.y,
        z: event.acceleration.z,
      }
      : null,
    accelerationIncludingGravity,
    rotationRate: event.rotationRate
      ? {
        alpha: event.rotationRate.alpha,
        beta: event.rotationRate.beta,
        gamma: event.rotationRate.gamma,
      }
      : null,
    interval: event.interval,
  };
  renderSensorOutput();
};

const requestSensorPermission = async (permissionTarget) => {
  if (typeof permissionTarget?.requestPermission !== "function") {
    return "granted";
  }

  return permissionTarget.requestPermission();
};

const stopSensorsFlow = () => {
  window.removeEventListener("deviceorientation", handleOrientation);
  window.removeEventListener("devicemotion", handleMotion);
  sensorsActive = false;
  sensorsButton.textContent = "Start sensors";
  append("device sensors stopped");
};

const startSensorsFlow = async () => {
  if (
    typeof window.DeviceOrientationEvent === "undefined"
    && typeof window.DeviceMotionEvent === "undefined"
  ) {
    throw new Error("Device orientation and motion APIs are not supported in this browser.");
  }

  const [orientationPermission, motionPermission] = await Promise.all([
    requestSensorPermission(window.DeviceOrientationEvent),
    requestSensorPermission(window.DeviceMotionEvent),
  ]);

  if (
    orientationPermission !== "granted"
    || motionPermission !== "granted"
  ) {
    throw new Error(
      `Sensor permission denied (orientation=${orientationPermission}, motion=${motionPermission})`,
    );
  }

  orientationState = null;
  motionState = null;
  renderSensorOutput();
  window.addEventListener("deviceorientation", handleOrientation);
  window.addEventListener("devicemotion", handleMotion);
  sensorsActive = true;
  sensorsButton.textContent = "Stop sensors";
  append("device sensors started; streaming locally to textbox");
};

const getTopK = (values, limit = 3) => {
  return values
    .map((value, index) => ({ value, index }))
    .sort((left, right) => right.value - left.value)
    .slice(0, limit);
};

const ensureVideoCvCanvas = () => {
  if (!videoCvCanvas) {
    videoCvCanvas = document.createElement("canvas");
    videoCvContext = videoCvCanvas.getContext("2d", { willReadFrequently: true });
  }

  if (!videoCvContext) {
    throw new Error("Unable to create 2D canvas context for video preprocessing.");
  }

  return videoCvContext;
};

const ensureVideoOverlayContext = () => {
  if (!videoOverlayContext) {
    videoOverlayContext = videoOutputCanvas.getContext("2d");
  }

  if (!videoOverlayContext) {
    throw new Error("Unable to create video output canvas context.");
  }

  return videoOverlayContext;
};

const selectVideoModelInputName = (session) => {
  const inputNames = Array.isArray(session?.inputNames) ? session.inputNames : [];
  if (!inputNames.length) {
    return null;
  }

  const ranked = inputNames
    .map((name) => {
      const metadata = session?.inputMetadata?.[name];
      const dimensions = Array.isArray(metadata?.dimensions) ? metadata.dimensions : [];
      const normalizedName = String(name).toLowerCase();
      let score = 0;

      if (dimensions.length === 4) {
        score += 100;
      } else if (dimensions.length === 3) {
        score += 40;
      }

      if (
        normalizedName.includes("pixel")
        || normalizedName.includes("image")
        || normalizedName.includes("images")
        || normalizedName.includes("input")
      ) {
        score += 25;
      }

      if (normalizedName.includes("mask") || normalizedName.includes("token")) {
        score -= 50;
      }

      return { name, score };
    })
    .sort((left, right) => right.score - left.score);

  return ranked[0]?.name ?? inputNames[0];
};

const selectVideoModelOutputName = (session) => {
  const outputNames = Array.isArray(session?.outputNames) ? session.outputNames : [];
  if (!outputNames.length) {
    return null;
  }

  const ranked = outputNames
    .map((name) => {
      const normalizedName = String(name).toLowerCase();
      let score = 0;
      if (normalizedName.includes("box")) {
        score += 100;
      }
      if (normalizedName.includes("logit") || normalizedName.includes("score")) {
        score += 40;
      }
      return { name, score };
    })
    .sort((left, right) => right.score - left.score);

  return ranked[0]?.name ?? outputNames[0];
};

const resolveVideoModelLayout = () => {
  if (!videoCvSession || !videoCvInputName) {
    throw new Error("Video CV model is not loaded.");
  }

  const metadata = videoCvSession.inputMetadata?.[videoCvInputName];
  const dataType = metadata?.type ?? "float32";
  if (dataType !== "float32" && dataType !== "uint8") {
    throw new Error(`Unsupported video model input type: ${dataType}`);
  }

  const rawDimensions = Array.isArray(metadata?.dimensions)
    ? metadata.dimensions
    : [];
  const dimensions = rawDimensions.length === 4
    ? rawDimensions
    : rawDimensions.length === 3
    ? [1, ...rawDimensions]
    : [1, 3, VIDEO_FALLBACK_INPUT_SIZE, VIDEO_FALLBACK_INPUT_SIZE];

  const resolved = dimensions.map((dimension, index) => {
    if (typeof dimension === "number" && Number.isFinite(dimension) && dimension > 0) {
      return dimension;
    }

    if (index === 0) {
      return 1;
    }

    if (index === 1 && dimensions.length === 4) {
      const inputName = String(videoCvInputName).toLowerCase();
      if (!inputName.includes("nhwc")) {
        return 3;
      }
    }

    return VIDEO_FALLBACK_INPUT_SIZE;
  });

  const secondDimension = resolved[1];
  const lastDimension = resolved[3];
  const inputName = String(videoCvInputName).toLowerCase();
  const channelsFirst = inputName.includes("nhwc")
    ? false
    : secondDimension === 1
      || secondDimension === 3
      || ((lastDimension !== 1 && lastDimension !== 3) && !inputName.includes("image_embeddings"));
  if (channelsFirst) {
    const [, channels, height, width] = resolved;
    if (channels !== 1 && channels !== 3) {
      throw new Error(`Unsupported channel count for NCHW image input: ${channels}`);
    }

    return {
      dataType,
      channels,
      width,
      height,
      tensorDimensions: [1, channels, height, width],
      layout: "nchw",
    };
  }

  const [, height, width, channels] = resolved;
  if (channels !== 1 && channels !== 3) {
    throw new Error(`Unsupported channel count for NHWC image input: ${channels}`);
  }

  return {
    dataType,
    channels,
    width,
    height,
    tensorDimensions: [1, height, width, channels],
    layout: "nhwc",
  };
};

const buildVideoInputTensor = () => {
  if (!videoCapture || !videoCvSession || !videoCvInputName) {
    throw new Error("Video capture or model session is unavailable.");
  }

  if (!videoPreview.videoWidth || !videoPreview.videoHeight) {
    throw new Error("Video stream is not ready yet.");
  }

  const {
    dataType,
    channels,
    width,
    height,
    tensorDimensions,
    layout,
  } = resolveVideoModelLayout();
  const context = ensureVideoCvCanvas();
  videoCvCanvas.width = width;
  videoCvCanvas.height = height;
  context.drawImage(videoPreview, 0, 0, width, height);

  const rgba = context.getImageData(0, 0, width, height).data;
  const elementCount = width * height * channels;
  const tensorData = dataType === "uint8"
    ? new Uint8Array(elementCount)
    : new Float32Array(elementCount);

  for (let pixelIndex = 0; pixelIndex < width * height; pixelIndex += 1) {
    const rgbaIndex = pixelIndex * 4;
    const red = rgba[rgbaIndex];
    const green = rgba[rgbaIndex + 1];
    const blue = rgba[rgbaIndex + 2];

    if (channels === 1) {
      const grayscale = Math.round(0.299 * red + 0.587 * green + 0.114 * blue);
      tensorData[pixelIndex] = dataType === "uint8" ? grayscale : grayscale / 255;
      continue;
    }

    if (layout === "nchw") {
      const planeSize = width * height;
      if (dataType === "uint8") {
        tensorData[pixelIndex] = red;
        tensorData[pixelIndex + planeSize] = green;
        tensorData[pixelIndex + 2 * planeSize] = blue;
      } else {
        tensorData[pixelIndex] = red / 255;
        tensorData[pixelIndex + planeSize] = green / 255;
        tensorData[pixelIndex + 2 * planeSize] = blue / 255;
      }
      continue;
    }

    const tensorIndex = pixelIndex * channels;
    if (dataType === "uint8") {
      tensorData[tensorIndex] = red;
      tensorData[tensorIndex + 1] = green;
      tensorData[tensorIndex + 2] = blue;
    } else {
      tensorData[tensorIndex] = red / 255;
      tensorData[tensorIndex + 1] = green / 255;
      tensorData[tensorIndex + 2] = blue / 255;
    }
  }

  return new window.ort.Tensor(dataType, tensorData, tensorDimensions);
};

const looksLikeBoxes = (tensor) => {
  if (!tensor?.dims || !tensor?.data) {
    return false;
  }

  const dims = tensor.dims.filter((dimension) => Number.isFinite(dimension));
  const values = Array.from(tensor.data ?? []);
  const lastDimension = dims[dims.length - 1];
  return values.length >= 4 && (lastDimension === 4 || lastDimension === 6 || lastDimension === 7);
};

const flattenFinite = (tensor) => {
  return Array.from(tensor?.data ?? []).map(Number).filter((value) => Number.isFinite(value));
};

const normalizeBox = (boxValues, format = "xyxy") => {
  if (boxValues.length < 4) {
    return null;
  }

  let x1;
  let y1;
  let x2;
  let y2;
  if (format === "cxcywh") {
    const [centerX, centerY, width, height] = boxValues;
    x1 = centerX - width / 2;
    y1 = centerY - height / 2;
    x2 = centerX + width / 2;
    y2 = centerY + height / 2;
  } else {
    [x1, y1, x2, y2] = boxValues;
  }

  if (x2 < x1) {
    [x1, x2] = [x2, x1];
  }
  if (y2 < y1) {
    [y1, y2] = [y2, y1];
  }

  const normalized = [x1, y1, x2, y2].map((value) => (
    value > 1.5 ? value : Math.max(0, Math.min(1, value))
  ));

  return normalized;
};

const softmax = (logits) => {
  const maxLogit = Math.max(...logits);
  const scores = logits.map((l) => Math.exp(l - maxLogit));
  const sumScores = scores.reduce((a, b) => a + b, 0);
  return scores.map((s) => s / sumScores);
};

const findDetectionTensor = (entries, patterns, predicate = () => true) => {
  return entries.find(([name, tensor]) => {
    const normalizedName = String(name).toLowerCase();
    return patterns.some((pattern) => pattern.test(normalizedName)) && predicate(tensor);
  }) ?? null;
};

const decodeHuggingFaceDetectionOutputs = (entries) => {
  const boxesEntry = findDetectionTensor(
    entries,
    [/pred_boxes/, /boxes?/, /bbox/],
    (tensor) => (Array.isArray(tensor?.dims) ? tensor.dims[tensor.dims.length - 1] : null) === 4,
  );
  const logitsEntry = findDetectionTensor(
    entries,
    [/logits/, /scores?/, /class/],
    (tensor) => (Array.isArray(tensor?.dims) ? tensor.dims[tensor.dims.length - 1] : 0) > 1,
  );

  if (!boxesEntry || !logitsEntry) {
    return null;
  }

  const [boxesName, boxesTensor] = boxesEntry;
  const [, logitsTensor] = logitsEntry;
  const rawBoxes = flattenFinite(boxesTensor);
  const rawLogits = flattenFinite(logitsTensor);
  const boxCount = Math.floor(rawBoxes.length / 4);
  const classCount = boxCount > 0 ? Math.floor(rawLogits.length / boxCount) : 0;
  if (boxCount <= 0 || classCount <= 1) {
    return null;
  }

  const usesCenterBoxes = /pred_boxes/.test(String(boxesName).toLowerCase());
  const detections = [];
  for (let index = 0; index < boxCount; index += 1) {
    const box = rawBoxes.slice(index * 4, index * 4 + 4);
    const logits = rawLogits.slice(index * classCount, index * classCount + classCount);
    const candidateLogits = logits.length > 1 ? logits.slice(0, -1) : logits;
    const probabilities = softmax(candidateLogits);
    const best = getTopK(probabilities, 1)[0];
    if (!best || best.value < VIDEO_RENDER_SCORE_THRESHOLD) {
      continue;
    }

    const normalizedBox = normalizeBox(box, usesCenterBoxes ? "cxcywh" : "xyxy");
    if (!normalizedBox) {
      continue;
    }

    detections.push({
      label: `class_${best.index}`,
      class_index: best.index,
      score: best.value,
      box: normalizedBox,
    });
  }

  if (!detections.length) {
    return {
      mode: "detection",
      detections: [],
      detected_class: "no_detection",
      class_index: -1,
      confidence: 0,
      probabilities: [],
      top_classes: [],
    };
  }

  detections.sort((left, right) => right.score - left.score);
  const best = detections[0];
  return {
    mode: "detection",
    detections,
    detected_class: best.label,
    class_index: best.class_index,
    confidence: best.score,
    probabilities: detections.map((entry) => entry.score),
    top_classes: detections.slice(0, 3).map((entry) => ({
      label: entry.label,
      index: entry.class_index,
      probability: entry.score,
    })),
  };
};

const decodeDetectionOutputs = (outputs) => {
  const entries = Object.entries(outputs);
  const huggingFaceSummary = decodeHuggingFaceDetectionOutputs(entries);
  if (huggingFaceSummary) {
    return huggingFaceSummary;
  }

  const boxesEntry = entries.find(([, tensor]) => looksLikeBoxes(tensor));

  if (!boxesEntry) {
    return null;
  }

  const [boxesName, boxesTensor] = boxesEntry;
  const boxDims = Array.isArray(boxesTensor.dims) ? boxesTensor.dims : [];
  const rawBoxes = flattenFinite(boxesTensor);
  const boxWidth = boxDims[boxDims.length - 1] ?? 4;
  const detectionCount = Math.floor(rawBoxes.length / boxWidth);
  if (detectionCount <= 0) {
    return null;
  }

  const scoresEntry = entries.find(([name, tensor]) =>
    name !== boxesName && flattenFinite(tensor).length >= detectionCount
  );
  const classEntry = entries.find(([name, tensor]) =>
    name !== boxesName && name !== scoresEntry?.[0] && flattenFinite(tensor).length >= detectionCount
  );
  const detections = [];
  const scoreValues = scoresEntry ? flattenFinite(scoresEntry[1]) : [];
  const classValues = classEntry ? flattenFinite(classEntry[1]) : [];

  for (let index = 0; index < detectionCount; index += 1) {
    const start = index * boxWidth;
    const row = rawBoxes.slice(start, start + boxWidth);
    const normalizedBox = normalizeBox(row);
    if (!normalizedBox) {
      continue;
    }

    let score = Number(scoreValues[index] ?? row[4] ?? row[5] ?? 1);
    if (!Number.isFinite(score)) {
      score = 1;
    }

    let classIndex = classValues[index];
    if (!Number.isFinite(classIndex)) {
      classIndex = row.length >= 6 ? row[5] : row.length >= 7 ? row[6] : index;
    }

    if (score < VIDEO_RENDER_SCORE_THRESHOLD) {
      continue;
    }

    detections.push({
      label: `class_${Math.round(classIndex)}`,
      class_index: Math.round(classIndex),
      score,
      box: normalizedBox,
    });
  }

  if (!detections.length) {
    return {
      mode: "detection",
      detections: [],
      detected_class: "no_detection",
      class_index: -1,
      confidence: 0,
      probabilities: [],
      top_classes: [],
    };
  }

  detections.sort((left, right) => right.score - left.score);
  const best = detections[0];
  return {
    mode: "detection",
    detections,
    detected_class: best.label,
    class_index: best.class_index,
    confidence: best.score,
    probabilities: detections.map((entry) => entry.score),
    top_classes: detections.slice(0, 3).map((entry) => ({
      label: entry.label,
      index: entry.class_index,
      probability: entry.score,
    })),
  };
};

const decodeClassificationOutputs = (output) => {
  const values = Array.from(output?.data ?? []);
  if (values.length === 0) {
    throw new Error("Video model returned an empty output tensor.");
  }

  if (values.length === 1) {
    return {
      mode: "classification",
      detections: [],
      detected_class: "scalar_output",
      class_index: 0,
      confidence: Number(values[0]),
      probabilities: values,
      top_classes: [{ label: "scalar_output", index: 0, probability: Number(values[0]) }],
    };
  }

  const probabilities = softmax(values);
  const ranked = getTopK(probabilities, 3);
  const best = ranked[0];

  return {
    mode: "classification",
    detections: [],
    detected_class: `class_${best.index}`,
    class_index: best.index,
    confidence: best.value,
    probabilities,
    top_classes: ranked.map(({ index, value }) => ({
      label: `class_${index}`,
      index,
      probability: value,
      logit: values[index],
    })),
  };
};

const summarizeVideoOutput = (outputMap) => {
  const detectionSummary = decodeDetectionOutputs(outputMap);
  if (detectionSummary) {
    return detectionSummary;
  }

  const primaryOutput = outputMap[videoCvOutputName];
  const primaryValues = Array.from(primaryOutput?.data ?? []);
  if (primaryValues.length > 0 && primaryValues.length <= 4096) {
    return decodeClassificationOutputs(primaryOutput);
  }

  return {
    mode: "passthrough",
    detections: [],
    detected_class: "unrecognized_output",
    class_index: -1,
    confidence: 0,
    probabilities: [],
    top_classes: [],
  };
};

const drawOverlayText = (context, lines) => {
  if (!lines.length) {
    return;
  }

  context.font = "18px ui-monospace, monospace";
  const lineHeight = 24;
  const width = Math.max(...lines.map((line) => context.measureText(line).width), 0) + 20;
  const height = lines.length * lineHeight + 12;
  context.fillStyle = "rgba(24, 32, 40, 0.72)";
  context.fillRect(12, 12, width, height);
  context.fillStyle = "#fffdfa";
  lines.forEach((line, index) => {
    context.fillText(line, 22, 36 + index * lineHeight);
  });
};

const renderVideoOutputFrame = () => {
  videoRenderFrameId = null;

  if (!videoOutputVisible || !videoCapture || !videoPreview.videoWidth || !videoPreview.videoHeight) {
    return;
  }

  const context = ensureVideoOverlayContext();
  const width = videoPreview.videoWidth;
  const height = videoPreview.videoHeight;
  if (videoOutputCanvas.width !== width || videoOutputCanvas.height !== height) {
    videoOutputCanvas.width = width;
    videoOutputCanvas.height = height;
  }

  context.drawImage(videoPreview, 0, 0, width, height);

  if (lastVideoInferenceSummary?.mode === "detection") {
    context.lineWidth = 3;
    context.font = "16px ui-monospace, monospace";
    lastVideoInferenceSummary.detections.forEach((entry) => {
      const [x1, y1, x2, y2] = entry.box;
      const left = x1 <= 1 ? x1 * width : x1;
      const top = y1 <= 1 ? y1 * height : y1;
      const right = x2 <= 1 ? x2 * width : x2;
      const bottom = y2 <= 1 ? y2 * height : y2;
      const boxWidth = Math.max(1, right - left);
      const boxHeight = Math.max(1, bottom - top);

      context.strokeStyle = "#ef8f35";
      context.strokeRect(left, top, boxWidth, boxHeight);

      const label = `${entry.label} ${(entry.score * 100).toFixed(1)}%`;
      const textWidth = context.measureText(label).width + 10;
      context.fillStyle = "#182028";
      context.fillRect(left, Math.max(0, top - 24), textWidth, 22);
      context.fillStyle = "#fffdfa";
      context.fillText(label, left + 5, Math.max(16, top - 8));
    });
  } else if (lastVideoInferenceSummary?.mode === "classification") {
    drawOverlayText(context, [
      `classification: ${lastVideoInferenceSummary.detected_class}`,
      `confidence: ${(lastVideoInferenceSummary.confidence * 100).toFixed(1)}%`,
    ]);
  } else if (lastVideoInferenceSummary?.mode === "passthrough") {
    drawOverlayText(context, [
      "output mode: passthrough",
      "model output not recognized as detection or classification",
    ]);
  }

  videoRenderFrameId = window.requestAnimationFrame(renderVideoOutputFrame);
};

const syncVideoOutputView = () => {
  videoOutputCanvas.hidden = !videoOutputVisible || !videoCapture;
  videoOutputButton.textContent = videoOutputVisible ? "Hide video output" : "Show video output";

  if (!videoOutputVisible || !videoCapture) {
    if (videoRenderFrameId !== null) {
      window.cancelAnimationFrame(videoRenderFrameId);
      videoRenderFrameId = null;
    }
    updateVideoStatus();
    return;
  }

  if (videoRenderFrameId === null) {
    videoRenderFrameId = window.requestAnimationFrame(renderVideoOutputFrame);
  }
  updateVideoStatus();
};

const stopVideoCvLoop = () => {
  if (videoCvLoopId !== null) {
    window.clearInterval(videoCvLoopId);
    videoCvLoopId = null;
  }
  lastVideoCvLabel = null;
  updateVideoStatus();
};

const inferVideoPrediction = async () => {
  if (
    !videoCapture
    || !videoCvSession
    || !videoCvInputName
    || !videoCvOutputName
    || videoCvInferencePending
  ) {
    return;
  }

  const now = Date.now();
  if (now - lastVideoInferenceAt < VIDEO_INFERENCE_INTERVAL_MS) {
    return;
  }

  videoCvInferencePending = true;
  lastVideoInferenceAt = now;

  try {
    const input = buildVideoInputTensor();
    const outputMap = await videoCvSession.run({ [videoCvInputName]: input });
    const output = outputMap[videoCvOutputName];
    const summary = summarizeVideoOutput(outputMap);
    const labelChanged = summary.detected_class !== lastVideoCvLabel;
    lastVideoCvLabel = summary.detected_class;
    lastVideoInferenceSummary = summary;

    updateVideoStatus([
      `output mode: ${summary.mode}`,
      `prediction: ${summary.detected_class}`,
      `confidence: ${summary.confidence.toFixed(4)}`,
      ...(
        summary.mode === "detection"
          ? [
            `detections: ${summary.detections.length}`,
            ...summary.detections.slice(0, 3).map(
              (entry) =>
                `${entry.label}: score=${entry.score.toFixed(4)} box=${
                  entry.box.map((value) => value.toFixed(3)).join(",")
                }`,
            ),
          ]
          : [
            "top classes:",
            ...summary.top_classes.map(
              (entry) =>
                `${entry.label}: p=${entry.probability.toFixed(4)} logit=${
                  Number(entry.logit ?? entry.probability).toFixed(4)
                }`,
            ),
          ]
      ),
      `frame: ${videoPreview.videoWidth}x${videoPreview.videoHeight}`,
      `processed at: ${new Date().toLocaleTimeString()}`,
    ]);
    syncVideoOutputView();

    sendClientEvent("video_cv", "inference", {
      mode: summary.mode,
      detected_class: summary.detected_class,
      class_index: summary.class_index,
      confidence: summary.confidence,
      probabilities: summary.probabilities,
      top_classes: summary.top_classes,
      detections: summary.detections,
      changed: labelChanged,
      processed_at: new Date().toISOString(),
      model_path: VIDEO_MODEL_PATH,
      input_name: videoCvInputName,
      output_name: videoCvOutputName,
      input_dimensions: videoCvSession.inputMetadata?.[videoCvInputName]?.dimensions ?? [],
      output_dimensions: Array.isArray(output?.dims) ? output.dims : [],
      source_resolution: {
        width: videoPreview.videoWidth,
        height: videoPreview.videoHeight,
      },
    });
  } catch (error) {
    lastVideoInferenceSummary = {
      mode: "passthrough",
      detections: [],
      detected_class: "inference_error",
      class_index: -1,
      confidence: 0,
      probabilities: [],
      top_classes: [],
    };
    updateVideoStatus([
      `inference error: ${error instanceof Error ? error.message : String(error)}`,
    ]);
    console.error(error);
  } finally {
    videoCvInferencePending = false;
  }
};

const syncVideoCvLoop = () => {
  if (videoCapture && videoCvSession) {
    if (videoCvLoopId === null) {
      videoCvLoopId = window.setInterval(() => {
        void inferVideoPrediction();
      }, VIDEO_INFERENCE_INTERVAL_MS);
    }
    updateVideoStatus([
      "browser-side webcam inference active",
      "results are sent to the backend over the websocket.",
    ]);
    return;
  }

  stopVideoCvLoop();
  lastVideoInferenceSummary = null;
  updateVideoStatus([
    videoCvSession
      ? "model loaded; start video capture to begin inference."
      : `model file: ${VIDEO_MODEL_PATH}`,
  ]);
};

renderSensorOutput();
updateVideoStatus([
  `model file: ${VIDEO_MODEL_PATH}`,
  "load the model, then start video capture to process frames in-browser.",
]);

const wsProtocol = window.location.protocol === "https:" ? "wss:" : "ws:";
const wsUrl = `${wsProtocol}//${window.location.host}/ws`;
const retainedAgentId = readStoredAgentId();

logEl.textContent = `Initializing WASM from /pkg/et_ws_wasm_agent_bg.wasm\nWebSocket endpoint: ${wsUrl}`;
updateAgentCard(
  retainedAgentId
    ? "Found retained agent ID in local storage. It will be re-used on connect."
    : "No retained agent ID found. Waiting for server assignment.",
  retainedAgentId,
);

try {
  try {
    await populateModuleDropdown();
  } catch (error) {
    append(`Module discovery failed: ${describeError(error)}`);
    console.error("populateModuleDropdown error:", error);
  }

  await init();
  initTracing();

  const config = new WsClientConfig(wsUrl);
  const client = new WsClient(config);

  sendClientEvent = (capability, action, details) => {
    const payload = JSON.stringify({
      type: "client_event",
      capability,
      action,
      details,
    });

    try {
      client.send(payload);
    } catch (error) {
      append(`ws send error: ${error instanceof Error ? error.message : String(error)}`);
      console.error(error);
    }
  };

  client.set_on_state_change((state) => {
    append(`state: ${state}`);
    if (state === "connecting") {
      updateAgentCard("Connecting to websocket server...", client.get_client_id() || readStoredAgentId());
    } else if (state === "connected") {
      updateAgentCard(
        "Socket connected. Waiting for server identity acknowledgement...",
        client.get_client_id() || readStoredAgentId(),
      );
    } else if (state === "reconnecting") {
      updateAgentCard(
        "Disconnected. Trying to re-use retained agent ID...",
        client.get_client_id() || readStoredAgentId(),
      );
    } else if (state === "disconnected") {
      updateAgentCard(
        "Socket disconnected. Retained agent ID will be re-used on next connect.",
        client.get_client_id() || readStoredAgentId(),
      );
    }
  });
  client.set_on_message((message) => {
    append(`message: ${message}`);
    handleProtocolMessage(message);
  });

  client.connect();
  updateAgentCard(
    retainedAgentId
      ? "Attempting websocket connect with retained agent ID from local storage."
      : "Attempting first websocket connect. Waiting for server-assigned agent ID.",
    client.get_client_id() || retainedAgentId,
  );
  append(`client_id: ${client.get_client_id() || "(awaiting server assignment)"}`);

  micButton.addEventListener("click", async () => {
    try {
      if (microphone) {
        microphone.stop();
        microphone = null;
        micButton.textContent = "Start microphone";
        delete window.microphone;
        append("microphone stopped");
        sendClientEvent("microphone", "stopped", { track_count: 0 });
        return;
      }

      microphone = await MicrophoneAccess.request();
      micButton.textContent = "Stop microphone";
      append(`microphone granted: ${microphone.trackCount()} audio track(s)`);
      window.microphone = microphone;
      sendClientEvent("microphone", "started", {
        track_count: microphone.trackCount(),
      });
    } catch (error) {
      append(`microphone error: ${error instanceof Error ? error.message : String(error)}`);
      sendClientEvent("microphone", "error", {
        message: error instanceof Error ? error.message : String(error),
      });
      console.error(error);
    }
  });

  videoButton.addEventListener("click", async () => {
    try {
      if (videoCapture) {
        videoCapture.stop();
        videoCapture = null;
        videoPreview.srcObject = null;
        videoPreview.hidden = true;
        videoButton.textContent = "Start video";
        delete window.videoCapture;
        syncVideoCvLoop();
        syncVideoOutputView();
        append("video stopped");
        sendClientEvent("video", "stopped", { track_count: 0 });
        return;
      }

      videoCapture = await VideoCapture.request();
      videoPreview.srcObject = videoCapture.rawStream();
      videoPreview.hidden = false;
      videoButton.textContent = "Stop video";
      append(`video granted: ${videoCapture.trackCount()} video track(s)`);
      window.videoCapture = videoCapture;
      syncVideoCvLoop();
      syncVideoOutputView();
      sendClientEvent("video", "started", {
        track_count: videoCapture.trackCount(),
      });
    } catch (error) {
      append(`video error: ${error instanceof Error ? error.message : String(error)}`);
      sendClientEvent("video", "error", {
        message: error instanceof Error ? error.message : String(error),
      });
      console.error(error);
    }
  });

  bluetoothButton.addEventListener("click", async () => {
    try {
      bluetoothDevice = await BluetoothAccess.request();
      append(
        `bluetooth selected: name=${bluetoothDevice.name()} id=${bluetoothDevice.id()}`,
      );
      window.bluetoothDevice = bluetoothDevice;
      sendClientEvent("bluetooth", "selected", {
        name: bluetoothDevice.name(),
        id: bluetoothDevice.id(),
        gatt_connected: bluetoothDevice.gattConnected(),
      });
    } catch (error) {
      append(`bluetooth error: ${error instanceof Error ? error.message : String(error)}`);
      sendClientEvent("bluetooth", "error", {
        message: error instanceof Error ? error.message : String(error),
      });
      console.error(error);
    }
  });

  geolocationButton.addEventListener("click", async () => {
    try {
      const location = await GeolocationReading.request();
      append(
        `geolocation: lat=${location.latitude()} lon=${location.longitude()} accuracy=${location.accuracyMeters()}m`,
      );
      sendClientEvent("geolocation", "reading", {
        latitude: location.latitude(),
        longitude: location.longitude(),
        accuracy_meters: location.accuracyMeters(),
      });
      window.locationReading = location;
    } catch (error) {
      append(`geolocation error: ${error instanceof Error ? error.message : String(error)}`);
      sendClientEvent("geolocation", "error", {
        message: error instanceof Error ? error.message : String(error),
      });
      console.error(error);
    }
  });

  graphicsButton.addEventListener("click", () => {
    try {
      const graphics = GraphicsSupport.detect();
      append(
        `graphics: webgl=${graphics.webglSupported()} `
          + `webgl2=${graphics.webgl2Supported()} `
          + `webgpu=${graphics.webgpuSupported()} `
          + `webnn=${graphics.webnnSupported()}`,
      );
      sendClientEvent("graphics", "detected", {
        webgl_supported: graphics.webglSupported(),
        webgl2_supported: graphics.webgl2Supported(),
        webgpu_supported: graphics.webgpuSupported(),
        webnn_supported: graphics.webnnSupported(),
      });
      window.graphicsSupport = graphics;
    } catch (error) {
      append(`graphics error: ${error instanceof Error ? error.message : String(error)}`);
      sendClientEvent("graphics", "error", {
        message: error instanceof Error ? error.message : String(error),
      });
      console.error(error);
    }
  });

  webgpuTestButton.addEventListener("click", async () => {
    try {
      const probe = await WebGpuProbeResult.test();
      append(
        `webgpu probe: adapter_found=${probe.adapterFound()} device_created=${probe.deviceCreated()}`,
      );
      sendClientEvent("webgpu", "probe", {
        adapter_found: probe.adapterFound(),
        device_created: probe.deviceCreated(),
      });
      window.webgpuProbe = probe;
    } catch (error) {
      append(`webgpu error: ${error instanceof Error ? error.message : String(error)}`);
      sendClientEvent("webgpu", "error", {
        message: error instanceof Error ? error.message : String(error),
      });
      console.error(error);
    }
  });

  gpuInfoButton.addEventListener("click", async () => {
    try {
      const gpuInfo = await GpuInfo.detect();
      append(
        `gpu info: source=${gpuInfo.source()} vendor=${gpuInfo.vendor()} `
          + `renderer=${gpuInfo.renderer()} architecture=${gpuInfo.architecture()} `
          + `description=${gpuInfo.description()}`,
      );
      sendClientEvent("gpu", "info", {
        source: gpuInfo.source(),
        vendor: gpuInfo.vendor(),
        renderer: gpuInfo.renderer(),
        architecture: gpuInfo.architecture(),
        description: gpuInfo.description(),
      });
      window.gpuInfo = gpuInfo;
    } catch (error) {
      append(`gpu info error: ${error instanceof Error ? error.message : String(error)}`);
      sendClientEvent("gpu", "error", {
        message: error instanceof Error ? error.message : String(error),
      });
      console.error(error);
    }
  });

  speechButton.addEventListener("click", async () => {
    if (speechListening && speechSession) {
      try {
        speechButton.disabled = true;
        speechButton.textContent = "Finishing...";
        await speechSession.stop();
        append("speech recognition finalizing");
      } catch (error) {
        append(`speech stop error: ${error instanceof Error ? error.message : String(error)}`);
      }
      return;
    }

    try {
      speechSession = new SpeechRecognitionSession();
      speechListening = true;
      speechButton.disabled = false;
      speechButton.textContent = "Stop speech";
      const speech = await speechSession.start();
      append(
        `speech: transcript="${speech.transcript()}" confidence=${speech.confidence()}`,
      );
      sendClientEvent("speech", "recognized", {
        transcript: speech.transcript(),
        confidence: speech.confidence(),
      });
      window.speechRecognitionResult = speech;
    } catch (error) {
      append(`speech error: ${error instanceof Error ? error.message : String(error)}`);
      sendClientEvent("speech", "error", {
        message: error instanceof Error ? error.message : String(error),
      });
      console.error(error);
    } finally {
      speechListening = false;
      speechButton.disabled = false;
      speechButton.textContent = "Recognize speech";
    }
  });

  nfcButton.addEventListener("click", async () => {
    try {
      nfcButton.disabled = true;
      nfcButton.textContent = "Scanning...";
      const scan = await NfcScanResult.scanOnce();
      append(`nfc: serial=${scan.serialNumber()} records=${scan.recordSummary()}`);
      sendClientEvent("nfc", "scanned", {
        serial_number: scan.serialNumber(),
        record_summary: scan.recordSummary(),
      });
      window.nfcScan = scan;
    } catch (error) {
      append(`nfc error: ${error instanceof Error ? error.message : String(error)}`);
      sendClientEvent("nfc", "error", {
        message: error instanceof Error ? error.message : String(error),
      });
      console.error(error);
    } finally {
      nfcButton.disabled = false;
      nfcButton.textContent = "Scan NFC";
    }
  });

  sensorsButton.addEventListener("click", async () => {
    try {
      if (sensorsActive) {
        stopSensorsFlow();
        return;
      }

      await startSensorsFlow();
    } catch (error) {
      append(`sensor error: ${describeError(error)}`);
      console.error(error);
    }
  });

  runModuleButton.addEventListener("click", async () => {
    const selectedModule = WORKFLOW_MODULES.get(moduleSelect.value);
    runModuleButton.disabled = true;
    moduleSelect.disabled = true;
    runModuleButton.textContent = selectedModule
      ? `Running ${selectedModule.label}...`
      : "Running module...";

    try {
      await runSelectedWorkflowModule();
    } catch (error) {
      append(`${selectedModule?.label ?? "workflow"} module error: ${describeError(error)}`);
      console.error(error);
    } finally {
      runModuleButton.disabled = false;
      moduleSelect.disabled = false;
      runModuleButton.textContent = "Run module";
    }
  });

  videoOutputButton.addEventListener("click", () => {
    videoOutputVisible = !videoOutputVisible;
    syncVideoOutputView();
  });

  window.client = client;
  window.sendAlive = () => client.send_alive();
  window.runWorkflowModule = (moduleKey) => {
    if (moduleKey && WORKFLOW_MODULES.has(moduleKey)) {
      moduleSelect.value = moduleKey;
    }
    return runSelectedWorkflowModule();
  };
  window.runHarModule = () => window.runWorkflowModule("har1");
  window.runFaceDetectionModule = () => window.runWorkflowModule("face-detection");
  window.runComm1Module = () => window.runWorkflowModule("comm1");
} catch (error) {
  append(`error: ${error instanceof Error ? error.message : String(error)}`);
  console.error(error);
}
