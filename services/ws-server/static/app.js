import init, {
  BluetoothAccess,
  GeolocationReading,
  GpuInfo,
  GraphicsSupport,
  MicrophoneAccess,
  NfcScanResult,
  SpeechRecognitionSession,
  VideoCapture,
  WebGpuProbeResult,
  WsClient,
  WsClientConfig,
} from "/pkg/et_ws_wasm_agent.js";

const logEl = document.getElementById("log");
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
const harButton = document.getElementById("har-button");
const harExportButton = document.getElementById("har-export-button");
const agentStatusEl = document.getElementById("agent-status");
const agentIdEl = document.getElementById("agent-id");
const sensorOutputEl = document.getElementById("sensor-output");
const harOutputEl = document.getElementById("har-output");
const videoPreview = document.getElementById("video-preview");
let microphone = null;
let videoCapture = null;
let bluetoothDevice = null;
let speechSession = null;
let speechListening = false;
let sensorsActive = false;
let orientationState = null;
let motionState = null;
let harSession = null;
let harInputName = null;
let harOutputName = null;
let harSampleBuffer = [];
let harInferencePending = false;
let lastInferenceAt = 0;
let harSamplerId = null;
let gravityEstimate = { x: 0, y: 0, z: 0 };
const HAR_SEQUENCE_LENGTH = 512;
const HAR_FEATURE_COUNT = 9;
const HAR_SAMPLE_INTERVAL_MS = 20;
const STANDARD_GRAVITY = 9.80665;
const GRAVITY_FILTER_ALPHA = 0.8;
const HAR_CLASS_LABELS = [
  "class_0",
  "class_1",
  "class_2",
  "class_3",
  "class_4",
  "class_5",
];
const HAR_CHANNEL_NAMES = [
  "body_acc_x",
  "body_acc_y",
  "body_acc_z",
  "body_gyro_x",
  "body_gyro_y",
  "body_gyro_z",
  "total_acc_x",
  "total_acc_y",
  "total_acc_z",
];
const STORED_AGENT_ID_KEY = "ws_wasm_agent.agent_id";
let currentAgentId = null;

const append = (line) => {
  logEl.textContent += `\n${line}`;
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

const degreesToRadians = (value) => (
  Number.isFinite(value) ? (value * Math.PI) / 180 : 0
);

const softmax = (values) => {
  if (!values.length) {
    return [];
  }

  const maxValue = Math.max(...values);
  const exps = values.map((value) => Math.exp(value - maxValue));
  const sum = exps.reduce((accumulator, value) => accumulator + value, 0);
  return exps.map((value) => value / sum);
};

const toG = (value) => (
  Number.isFinite(value) ? value / STANDARD_GRAVITY : 0
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

const setHarOutput = (lines) => {
  harOutputEl.value = Array.isArray(lines) ? lines.join("\n") : String(lines);
};

const updateHarStatus = (extraLines = []) => {
  const lines = [
    `model: ${harSession ? "loaded" : "not loaded"}`,
    `input: ${harInputName ?? "n/a"}`,
    `output: ${harOutputName ?? "n/a"}`,
    "layout: [batch, time, features]",
    `window: ${HAR_SEQUENCE_LENGTH}`,
    `features: ${HAR_FEATURE_COUNT}`,
    `buffered samples: ${harSampleBuffer.length}`,
  ];
  setHarOutput(lines.concat("", extraLines));
};

const getFeatureVector = () => {
  const totalAcceleration = motionState?.accelerationIncludingGravity ?? { x: 0, y: 0, z: 0 };
  const bodyAcceleration = {
    x: totalAcceleration.x - gravityEstimate.x,
    y: totalAcceleration.y - gravityEstimate.y,
    z: totalAcceleration.z - gravityEstimate.z,
  };

  return [
    toG(bodyAcceleration.x),
    toG(bodyAcceleration.y),
    toG(bodyAcceleration.z),
    degreesToRadians(motionState?.rotationRate?.beta),
    degreesToRadians(motionState?.rotationRate?.gamma),
    degreesToRadians(motionState?.rotationRate?.alpha),
    toG(totalAcceleration.x),
    toG(totalAcceleration.y),
    toG(totalAcceleration.z),
  ];
};

const flattenSamplesForModel = () => {
  return harSampleBuffer.slice(-HAR_SEQUENCE_LENGTH).flat();
};

const exportHarWindow = () => {
  if (harSampleBuffer.length < HAR_SEQUENCE_LENGTH) {
    throw new Error(`Need ${HAR_SEQUENCE_LENGTH} samples before export.`);
  }

  const samples = harSampleBuffer.slice(-HAR_SEQUENCE_LENGTH);
  const columns = [];

  for (let timeIndex = 0; timeIndex < HAR_SEQUENCE_LENGTH; timeIndex += 1) {
    for (const channelName of HAR_CHANNEL_NAMES) {
      columns.push(`t${timeIndex}_${channelName}`);
    }
  }

  const values = [];
  for (let timeIndex = 0; timeIndex < HAR_SEQUENCE_LENGTH; timeIndex += 1) {
    for (let channelIndex = 0; channelIndex < HAR_CHANNEL_NAMES.length; channelIndex += 1) {
      values.push(String(samples[timeIndex][channelIndex] ?? 0));
    }
  }

  const csv = `${columns.join(",")},true_label\n${values.join(",")},\n`;
  const blob = new Blob([csv], { type: "text/csv;charset=utf-8" });
  const url = URL.createObjectURL(blob);
  const link = document.createElement("a");
  link.href = url;
  link.download = "har_window.csv";
  link.click();
  URL.revokeObjectURL(url);
};

const inferHarPrediction = async () => {
  if (
    !harSession
    || !harInputName
    || !harOutputName
    || harInferencePending
    || harSampleBuffer.length < HAR_SEQUENCE_LENGTH
  ) {
    updateHarStatus();
    return;
  }

  const now = Date.now();
  if (now - lastInferenceAt < 250) {
    return;
  }

  harInferencePending = true;
  lastInferenceAt = now;

  try {
    const input = new window.ort.Tensor(
      "float32",
      Float32Array.from(flattenSamplesForModel()),
      [1, HAR_SEQUENCE_LENGTH, HAR_FEATURE_COUNT],
    );

    const result = await harSession.run({ [harInputName]: input });
    const output = result[harOutputName];
    const logits = Array.from(output.data ?? []);
    const probabilities = softmax(logits);
    const bestProbability = Math.max(...probabilities);
    const bestIndex = probabilities.indexOf(bestProbability);
    const allScores = probabilities.map((probability, index) => {
      const label = HAR_CLASS_LABELS[index] ?? `class_${index}`;
      const logit = logits[index] ?? 0;
      return `${label}: p=${probability.toFixed(4)} logit=${logit.toFixed(4)}`;
    });

    updateHarStatus([
      `prediction: ${HAR_CLASS_LABELS[bestIndex] ?? `class_${bestIndex}`}`,
      `confidence: ${bestProbability.toFixed(4)}`,
      "all classes:",
      ...allScores,
    ]);
  } catch (error) {
    updateHarStatus([
      `inference error: ${error instanceof Error ? error.message : String(error)}`,
    ]);
    console.error(error);
  } finally {
    harInferencePending = false;
  }
};

const pushHarSample = () => {
  if (!harSession || !sensorsActive || !motionState) {
    return;
  }

  harSampleBuffer.push(getFeatureVector());
  if (harSampleBuffer.length > HAR_SEQUENCE_LENGTH) {
    harSampleBuffer.shift();
  }

  void inferHarPrediction();
};

const stopHarSampler = () => {
  if (harSamplerId !== null) {
    window.clearInterval(harSamplerId);
    harSamplerId = null;
  }
};

const startHarSampler = () => {
  stopHarSampler();
  harSamplerId = window.setInterval(() => {
    pushHarSample();
  }, HAR_SAMPLE_INTERVAL_MS);
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

  if (accelerationIncludingGravity) {
    gravityEstimate = {
      x: GRAVITY_FILTER_ALPHA * gravityEstimate.x + (1 - GRAVITY_FILTER_ALPHA) * accelerationIncludingGravity.x,
      y: GRAVITY_FILTER_ALPHA * gravityEstimate.y + (1 - GRAVITY_FILTER_ALPHA) * accelerationIncludingGravity.y,
      z: GRAVITY_FILTER_ALPHA * gravityEstimate.z + (1 - GRAVITY_FILTER_ALPHA) * accelerationIncludingGravity.z,
    };
  }

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
  pushHarSample();
};

const requestSensorPermission = async (permissionTarget) => {
  if (typeof permissionTarget?.requestPermission !== "function") {
    return "granted";
  }

  return permissionTarget.requestPermission();
};

renderSensorOutput();
updateHarStatus([
  "local-only inference path",
  "model file: /static/models/human_activity_recognition.onnx",
]);

harExportButton.addEventListener("click", () => {
  try {
    exportHarWindow();
    append("exported current HAR window to har_window.csv");
  } catch (error) {
    append(`har export error: ${error instanceof Error ? error.message : String(error)}`);
    console.error(error);
  }
});

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
  await init();

  const config = new WsClientConfig(wsUrl);
  const client = new WsClient(config);

  const sendClientEvent = (capability, action, details) => {
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
        window.removeEventListener("deviceorientation", handleOrientation);
        window.removeEventListener("devicemotion", handleMotion);
        stopHarSampler();
        sensorsActive = false;
        sensorsButton.textContent = "Start sensors";
        append("device sensors stopped");
        return;
      }

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
      gravityEstimate = { x: 0, y: 0, z: 0 };
      harSampleBuffer = [];
      renderSensorOutput();
      window.addEventListener("deviceorientation", handleOrientation);
      window.addEventListener("devicemotion", handleMotion);
      startHarSampler();
      sensorsActive = true;
      sensorsButton.textContent = "Stop sensors";
      append("device sensors started; streaming locally to textbox");
    } catch (error) {
      append(`sensor error: ${error instanceof Error ? error.message : String(error)}`);
      console.error(error);
    }
  });

  harButton.addEventListener("click", async () => {
    try {
      if (!window.ort) {
        throw new Error("onnxruntime-web did not load.");
      }

      harButton.disabled = true;
      harButton.textContent = "Loading HAR...";
      updateHarStatus(["loading model..."]);

      harSession = await window.ort.InferenceSession.create(
        "/static/models/human_activity_recognition.onnx",
        {
          executionProviders: ["wasm"],
        },
      );

      harInputName = harSession.inputNames[0] ?? null;
      harOutputName = harSession.outputNames[0] ?? null;

      const inputMetadata = harInputName
        ? harSession.inputMetadata?.[harInputName]
        : null;
      const runtimeDimensions = Array.isArray(inputMetadata?.dimensions)
        ? inputMetadata.dimensions
        : [];

      harSampleBuffer = [];
      gravityEstimate = { x: 0, y: 0, z: 0 };
      harButton.textContent = "Reload HAR model";
      if (sensorsActive) {
        startHarSampler();
      }
      append(
        `har model loaded: input=${harInputName} output=${harOutputName} runtime_dims=${
          JSON.stringify(runtimeDimensions)
        } expected_dims=["batch",512,9]`,
      );
      updateHarStatus([
        `runtime dimensions: ${JSON.stringify(runtimeDimensions)}`,
        "expected dimensions: [batch, 512, 9]",
        "feature order: body_acc xyz, body_gyro xyz, total_acc xyz",
        "sampling target: 50 Hz from DeviceMotion events",
        "predictions remain browser-local and are not sent over the websocket.",
      ]);
    } catch (error) {
      harSession = null;
      harInputName = null;
      harOutputName = null;
      harSampleBuffer = [];
      stopHarSampler();
      updateHarStatus([
        `model load error: ${error instanceof Error ? error.message : String(error)}`,
      ]);
      append(`har error: ${error instanceof Error ? error.message : String(error)}`);
      console.error(error);
    } finally {
      harButton.disabled = false;
      harButton.textContent = harSession ? "Reload HAR model" : "Load HAR model";
    }
  });

  window.client = client;
  window.sendAlive = () => client.send_alive();
} catch (error) {
  append(`error: ${error instanceof Error ? error.message : String(error)}`);
  console.error(error);
}
