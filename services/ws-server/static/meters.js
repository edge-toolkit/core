// GPU utilisation meter overlay, backed by the stats-gl npm package (served at /modules/stats-gl).
//
// stats-gl's GPU panel needs WebGPU "timestamp-query" timestamps written around real GPU passes.
// This page owns no render loop, so a tiny fixed compute probe is dispatched every animation frame
// with stats-gl's timestampWrites: the panel then reports how long that constant workload takes on
// the device, which rises as other GPU work (module matmuls, onnxruntime-web inference) contends
// with it. Only the GPU panel is shown (no FPS/CPU), with a value line sized to span most of the
// panel width and without the flickering "(min-max)" range suffix. Browsers without WebGPU or
// without the timestamp-query feature get no overlay at all. The meter starts in the bottom-right
// corner (out of the way of the page content above it); it can be dragged anywhere, resized from
// its bottom-right corner handle, and closed with the corner x button, which also stops the probe
// and releases the WebGPU device.

import Stats from "/modules/stats-gl/dist/main.js";

const PROBE_SHADER = `
  @group(0) @binding(0) var<storage, read_write> data: array<f32>;

  @compute @workgroup_size(64)
  fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    var acc = data[id.x];
    for (var i = 0u; i < 256u; i = i + 1u) {
      acc = acc * 1.0000001 + 0.0000001;
    }
    data[id.x] = acc;
  }
`;

const PROBE_INVOCATIONS = 64;

// stats-gl's native panel is 90x48 CSS px; the meter starts at 2x that and the corner handle
// resizes it (aspect locked) between MIN_SCALE and MAX_SCALE.
const PANEL_BASE_WIDTH = 90;
const PANEL_BASE_HEIGHT = 48;
const START_SCALE = 2;
const MIN_SCALE = 1;
const MAX_SCALE = 8;
// Font height per panel device-pixel-ratio unit (stats-gl's own is 9). At bold 16, a worst-case
// "999.99 GPU" value line spans ~85 of the 90-unit panel width, so the text mostly fills the meter.
const FONT_PX = 16;

const requestTimestampDevice = async () => {
  if (!navigator.gpu) {
    return null;
  }
  const adapter = await navigator.gpu.requestAdapter();
  if (!adapter || !adapter.features.has("timestamp-query")) {
    return null;
  }
  return adapter.requestDevice({ requiredFeatures: ["timestamp-query"] });
};

const createProbe = async (device) => {
  const module = device.createShaderModule({ code: PROBE_SHADER });
  const pipeline = await device.createComputePipelineAsync({
    layout: "auto",
    compute: { module, entryPoint: "main" },
  });
  const buffer = device.createBuffer({
    size: PROBE_INVOCATIONS * 4,
    usage: GPUBufferUsage.STORAGE,
  });
  const bindGroup = device.createBindGroup({
    layout: pipeline.getBindGroupLayout(0),
    entries: [{ binding: 0, resource: { buffer } }],
  });
  return { bindGroup, device, pipeline };
};

const encodeProbePass = (stats, probe) => {
  const encoder = probe.device.createCommandEncoder();
  const pass = encoder.beginComputePass({ timestampWrites: stats.getTimestampWrites("render") });
  pass.setPipeline(probe.pipeline);
  pass.setBindGroup(0, probe.bindGroup);
  pass.dispatchWorkgroups(1);
  pass.end();
  stats.end(encoder);
  probe.device.queue.submit([encoder.finish()]);
};

// stats-gl hardcodes the panel's 90x48 layout, 15-unit text strip, and 9-unit font in the Panel
// constructor, so re-derive the metrics here for a given scale. Scaling the panel's device-pixel-
// ratio from a stored base (rather than stretching the canvas with CSS) keeps the meter crisp at
// every size, and the taller text strip plus FONT_PX make the value line span most of the panel
// width. The container is sized to match so the corner controls anchor to its edges and the drag
// clamp can read its offset size. update()/updateGraph() read all of these fields per frame, and
// never reset the font, so the overrides stick.
const applyMeterScale = (el, panel, scale) => {
  panel.basePR ??= panel.PR;
  const pr = panel.basePR * scale;
  panel.PR = pr;
  panel.WIDTH = PANEL_BASE_WIDTH * pr;
  panel.HEIGHT = PANEL_BASE_HEIGHT * pr;
  panel.TEXT_X = 3 * pr;
  panel.TEXT_Y = 3 * pr;
  panel.GRAPH_X = 3 * pr;
  panel.GRAPH_Y = 22 * pr;
  panel.GRAPH_WIDTH = 84 * pr;
  panel.GRAPH_HEIGHT = 23 * pr;
  panel.canvas.width = panel.WIDTH;
  panel.canvas.height = panel.HEIGHT;
  panel.canvas.style.width = `${PANEL_BASE_WIDTH * scale}px`;
  panel.canvas.style.height = `${PANEL_BASE_HEIGHT * scale}px`;
  el.style.width = `${PANEL_BASE_WIDTH * scale}px`;
  el.style.height = `${PANEL_BASE_HEIGHT * scale}px`;
  panel.initializeCanvas();
  panel.context.font = `bold ${FONT_PX * pr}px Helvetica,Arial,sans-serif`;
};

// panel.update draws a flickering " (min-max)" range as a separate fillText call after the value;
// dropping that call at the context level keeps one steady number without forking the panel
// drawing code. The wrapper survives resizes: reallocating the canvas resets context state but
// keeps the same context object, and with it this own-property override.
const suppressRangeSuffix = (panel) => {
  const fillText = panel.context.fillText.bind(panel.context);
  panel.context.fillText = (...args) => {
    if (!String(args[0]).trimStart().startsWith("(")) {
      fillText(...args);
    }
  };
};

// Move/up handlers for dragging and resizing sit on window rather than using setPointerCapture:
// capture can silently drop in embedded browser panes, stranding the gesture after one step, while
// window listeners keep receiving events wherever the cursor goes.
const makeDraggable = (el) => {
  let activePointerId = null;
  let offsetX = 0;
  let offsetY = 0;
  el.style.cursor = "grab";
  el.style.touchAction = "none";

  el.addEventListener("pointerdown", (event) => {
    activePointerId = event.pointerId;
    offsetX = event.clientX - el.offsetLeft;
    offsetY = event.clientY - el.offsetTop;
    el.style.cursor = "grabbing";
    event.preventDefault();
  });

  window.addEventListener("pointermove", (event) => {
    if (activePointerId === null || event.pointerId !== activePointerId) {
      return;
    }
    const x = event.clientX - offsetX;
    const y = event.clientY - offsetY;
    el.style.left = `${Math.min(Math.max(x, 0), window.innerWidth - el.offsetWidth)}px`;
    el.style.top = `${Math.min(Math.max(y, 0), window.innerHeight - el.offsetHeight)}px`;
  });

  const release = (event) => {
    if (event.pointerId === activePointerId) {
      activePointerId = null;
      el.style.cursor = "grab";
    }
  };
  window.addEventListener("pointerup", release);
  window.addEventListener("pointercancel", release);
};

// Corner controls swallow pointerdown so the drag handler underneath doesn't fire.
const addCloseButton = (el, onClose) => {
  const button = document.createElement("button");
  button.type = "button";
  button.textContent = "x";
  button.title = "Close GPU meter";
  Object.assign(button.style, {
    background: "rgba(0,0,0,0.55)",
    border: "0",
    borderRadius: "3px",
    color: "#ddd",
    cursor: "pointer",
    font: "bold 11px/16px sans-serif",
    height: "16px",
    padding: "0",
    position: "absolute",
    right: "2px",
    top: "2px",
    width: "16px",
  });
  button.addEventListener("pointerdown", (event) => event.stopPropagation());
  button.addEventListener("click", onClose);
  el.appendChild(button);
};

const addResizeHandle = (el, onScale) => {
  const handle = document.createElement("div");
  handle.title = "Resize GPU meter";
  Object.assign(handle.style, {
    background: "linear-gradient(135deg,transparent 50%,rgba(255,255,255,0.4) 50%)",
    bottom: "0",
    cursor: "nwse-resize",
    height: "14px",
    position: "absolute",
    right: "0",
    width: "14px",
  });
  let activePointerId = null;
  let startX = 0;
  let startWidth = 0;

  handle.addEventListener("pointerdown", (event) => {
    event.stopPropagation();
    event.preventDefault();
    activePointerId = event.pointerId;
    startX = event.clientX;
    startWidth = el.offsetWidth;
  });

  window.addEventListener("pointermove", (event) => {
    if (activePointerId === null || event.pointerId !== activePointerId) {
      return;
    }
    const scale = (startWidth + event.clientX - startX) / PANEL_BASE_WIDTH;
    onScale(Math.min(Math.max(scale, MIN_SCALE), MAX_SCALE));
  });

  const release = (event) => {
    if (event.pointerId === activePointerId) {
      activePointerId = null;
    }
  };
  window.addEventListener("pointerup", release);
  window.addEventListener("pointercancel", release);
  el.appendChild(handle);
};

const startMeters = async () => {
  let device = null;
  try {
    device = await requestTimestampDevice();
  } catch (error) {
    console.warn("meters: WebGPU device request failed:", error);
  }
  if (!device) {
    console.warn("meters: WebGPU timestamp queries unavailable, GPU meter not shown");
    return;
  }

  const stats = new Stats({ trackFPS: false, trackGPU: true });
  let probe = null;
  try {
    await stats.init(device);
    probe = await createProbe(device);
  } catch (error) {
    console.warn("meters: WebGPU probe setup failed, GPU meter not shown:", error);
    return;
  }

  const overlay = stats.dom;
  overlay.id = "gpu-utilisation-meter";
  applyMeterScale(overlay, stats.gpuPanel, START_SCALE);
  suppressRangeSuffix(stats.gpuPanel);
  document.body.appendChild(overlay);
  // stats-gl's own dom defaults to the top-left corner; place it bottom-right instead, clear of the
  // page content. `makeDraggable`'s pointermove handler always writes pixel left/top, so switching
  // away from right/bottom on the first drag is expected and fine.
  Object.assign(overlay.style, { position: "fixed", top: "auto", left: "auto", right: "24px", bottom: "32px" });
  makeDraggable(overlay);
  addResizeHandle(overlay, (scale) => applyMeterScale(overlay, stats.gpuPanel, scale));

  let frameHandle = 0;
  addCloseButton(overlay, () => {
    cancelAnimationFrame(frameHandle);
    probe = null;
    overlay.remove();
    device.destroy();
  });

  void device.lost.then((info) => {
    if (info.reason === "destroyed") {
      return;
    }
    probe = null;
    console.warn(`meters: WebGPU device lost (${info.reason}), GPU meter frozen`);
  });

  const frame = () => {
    stats.begin();
    if (probe) {
      encodeProbePass(stats, probe);
    } else {
      stats.end();
    }
    stats.update();
    frameHandle = requestAnimationFrame(frame);
  };
  frameHandle = requestAnimationFrame(frame);
};

await startMeters();
