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
const agentStatusEl = document.getElementById("agent-status");
const agentIdEl = document.getElementById("agent-id");
const sensorOutputEl = document.getElementById("sensor-output");
const videoPreview = document.getElementById("video-preview");

let microphone = null;
let videoCapture = null;
let bluetoothDevice = null;
let speechSession = null;
let speechListening = false;
let sensorsActive = false;
let orientationState = null;
let motionState = null;
let sendClientEvent = () => {};
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
  if (
    typeof loadedModule.is_running === "function"
    && loadedModule.is_running()
    && typeof loadedModule.stop === "function"
  ) {
    append(`${moduleConfig.label} module: calling stop()`);
    loadedModule.stop();
    append(`${moduleConfig.label} module stopped`);
    return;
  }

  append(`${moduleConfig.label} module: calling run()`);
  const runPromise = loadedModule.run();
  append(`${moduleConfig.label} module: run() started`);
  await runPromise;
  append(`${moduleConfig.label} module run() returned`);
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

renderSensorOutput();

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
