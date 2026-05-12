// pkg/demo/app.js
// Synthetic-dataset training demo for zig-te-train1.
//
// Architecture: this page instantiates the same wasm module the main UI does
// (via run() from ../et_ws_zig_te_train1.js), but with `ui: false` and an
// onReady controller so we drive the wasm directly from a loop here. Each
// "step" pushes a {type:"train", url:<blob>, label} envelope into the same
// pending queue the WebSocket loop would feed, then waits for the wasm's
// train_result reply. Evaluation re-uses the same loop with {type:"infer",
// url:<blob>}.
//
// The synthetic generator is a stripped-down port of webapp/js/app.js's
// person/scene drawers — flat shapes plus per-pixel Gaussian noise + speckles
// so the int8 feature extractor sees enough high-frequency content to
// produce non-saturated logits (otherwise the where_zeros gate in
// codegen/Source/genModel.c zeros gradient at the classifier head).

import { activeModel, MODEL_LABELS, run } from "../et_ws_zig_te_train1.js";

// ── Model indicator ────────────────────────────────────────────────────────
// The chip in index.html's H1 shows which backbone this demo session
// instantiated. activeModel() reads the same localStorage key + default
// the loader's resolveModel uses, so this can be populated synchronously
// at script load — well before the wasm worker boots and emits its `ready`
// envelope, no flash-of-unstyled-content. The floating UI's switchModel()
// writes that key when the user picks a new backbone, so opening the
// Demo afterwards shows the active selection here.
const _activeModelChip = () => {
  const el = document.getElementById("model-indicator");
  if (!el) return;
  const m = activeModel();
  const label = MODEL_LABELS[m] || m;
  el.textContent = label;
  el.title =
    `Backbone: ${label} (key: ${m}). Set by the floating UI's model dropdown — open the main page and switch from there.`;
};
_activeModelChip();

// ── Constants ──────────────────────────────────────────────────────────────
// Path C: 49KB sparse-bp training graph for mcunet-5fps. Inputs are 128×128
// RGB int8 (the paper's tinyengine training-tutorial default — see
// arXiv:2206.15472 Fig.10). NUM_CLASSES=10 since the head is the original
// 10-way classifier kept from the pretrained pkl.
const IMG_W = 128;
const IMG_H = 128;
const NUM_CLASSES = 10;
const BINARY_CLASSES = 2;
// MIT VWW labeling convention: the built-in head's class 0 is
// "no-person" (the more common class, bias45[0]=+866) and class 1 is
// "person" (bias45[1]=-778). Empirically confirmed by running the
// out-of-the-box pretrained head on real photos — without these
// constants, scene/person uploads come out 100% inverted.
const SCENE = 0; // model class 0 = no-person / background
const PERSON = 1; // model class 1 = person
const DEFAULT_SYNTH_TRAIN_PER_CLASS = 20;
const DEFAULT_SYNTH_TEST_PER_CLASS = 5;
const SAMPLE_PACK_MANIFEST_URL = "./sample-pack/manifest.json";
// Class-name labels indexed by class index (0=scene, 1=person), so
// CLASS_NAMES[PERSON] = "person" and CLASS_NAMES[SCENE] = "scene"
// still resolve to the user-friendly names.
const CLASS_NAMES = ["scene", "person"];
const CLASS_COLORS = ["#6f8fcf", "#6fc06f"];
function setClassNames(nameA, nameB) {
  CLASS_NAMES[PERSON] = nameA;
  CLASS_NAMES[SCENE] = nameB;
}

// ── State ──────────────────────────────────────────────────────────────────
const S = {
  dataset: [], // [{ canvas, label, lastPred, lastCorrect }]
  uploads: {
    train: [[], []], // user-provided canvases for class 0/1 training
    test: [[], []], // user-provided canvases for class 0/1 validation/test
  },
  controller: null, // { submit, shutdown } from run()
  pendingReply: null, // resolver for the in-flight wasm reply
  stop: false,
  epoch: 0,
  step: 0,
  trainLosses: [],
  epochAccs: [],
  arenaEpochAvgs: [],
  sramPeakOverall: 0,
  arenaPeakOverall: 0,
  // Running training-step accuracy across the entire training run. Each call
  // to runTraining() resets these. Updated after every train sample.
  runningCorrect: 0,
  runningTotal: 0,
  // Most recent held-out test accuracy. Set by evaluateTest at end of each
  // epoch; carries forward through the next epoch's per-step m-acc updates
  // so the displayed value persists instead of disappearing.
  lastTestAcc: null,
};

// ── DOM helpers ────────────────────────────────────────────────────────────
const $ = (id) => document.getElementById(id);
const logEl = $("log");
const galleryEl = $("gallery");

function syncDatasetPanelHeight() {
  const topRow = document.querySelector(".top-row");
  const controls = document.querySelector(".controls-panel");
  if (!topRow || !controls) return;
  const height = Math.ceil(controls.getBoundingClientRect().height);
  if (height > 0) topRow.style.setProperty("--controls-panel-height", `${height}px`);
}

const controlsPanel = document.querySelector(".controls-panel");
if (controlsPanel && "ResizeObserver" in window) {
  const controlsResizeObserver = new ResizeObserver(() => syncDatasetPanelHeight());
  controlsResizeObserver.observe(controlsPanel);
} else {
  window.addEventListener("resize", syncDatasetPanelHeight);
}
requestAnimationFrame(syncDatasetPanelHeight);
window.addEventListener("resize", syncDatasetPanelHeight);

function log(msg) {
  logEl.textContent += msg + "\n";
  logEl.scrollTop = logEl.scrollHeight;
}

function setMetric(id, val) {
  $(id).textContent = val;
}

function fmtKb(bytes) {
  return `${(bytes / 1024).toFixed(1)} KB`;
}

function setAgentStatus(msg, ok = false) {
  const el = $("agent-status");
  el.textContent = msg;
  el.classList.toggle("ok", ok);
}

function clampIntInput(id, fallback, min, max) {
  const el = $(id);
  const raw = el ? parseInt(el.value, 10) : NaN;
  const value = Number.isFinite(raw) ? Math.max(min, Math.min(max, raw)) : fallback;
  if (el && String(value) !== el.value) el.value = String(value);
  return value;
}

function datasetCounts() {
  let train = 0;
  let test = 0;
  let fileTrain = 0;
  let fileTest = 0;
  for (const item of S.dataset) {
    if (item.isTest) test++;
    else train++;
    if (item.source === "file") {
      if (item.isTest) fileTest++;
      else fileTrain++;
    }
  }
  return {
    train,
    test,
    fileTrain,
    fileTest,
    total: train + test,
    synthTrain: train - fileTrain,
    synthTest: test - fileTest,
  };
}

function updateDatasetSummary() {
  const counts = datasetCounts();
  const title = $("dataset-title");
  if (title) {
    title.textContent = `Dataset (${counts.total} images: ${counts.train} train + ${counts.test} validation/test)`;
  }
  const summary = $("dataset-summary");
  if (summary) {
    summary.innerHTML = `Class&nbsp;0 (<code>${CLASS_NAMES[PERSON]}</code>) and Class&nbsp;1 `
      + `(<code>${CLASS_NAMES[SCENE]}</code>) include `
      + `${counts.synthTrain} generated + ${counts.fileTrain} uploaded training images, `
      + `and ${counts.synthTest} generated + ${counts.fileTest} uploaded validation/test images. `
      + `Validation/test tiles use a yellow dashed outline and prefix <code>V</code>. Tile border: `
      + `<span style="color:#6fc06f;">green</span> correct, `
      + `<span style="color:#d4a0a0;">red</span> wrong.`;
  }
  const labels = [
    ["upload-train-a-label", PERSON],
    ["upload-test-a-label", PERSON],
    ["upload-train-b-label", SCENE],
    ["upload-test-b-label", SCENE],
  ];
  for (const [id, idx] of labels) {
    const el = $(id);
    if (el) el.textContent = CLASS_NAMES[idx];
  }
  const fileCounts = {
    "upload-train-a-count": S.uploads.train[PERSON].length,
    "upload-train-b-count": S.uploads.train[SCENE].length,
    "upload-test-a-count": S.uploads.test[PERSON].length,
    "upload-test-b-count": S.uploads.test[SCENE].length,
  };
  for (const [id, count] of Object.entries(fileCounts)) {
    const el = $(id);
    if (el) el.textContent = `${count} file${count === 1 ? "" : "s"}`;
  }
}

// ── Synthetic dataset generation ───────────────────────────────────────────
// Seeded LCG so the same seed produces the same dataset every time.
function makeRng(seed) {
  let s = seed >>> 0;
  return () => {
    s = (Math.imul(1664525, s) + 1013904223) >>> 0;
    return s / 0xFFFFFFFF;
  };
}

// Per-pixel Gaussian noise + diagonal chroma drift. Without high-frequency
// content the model's late layers all saturate and gradients vanish.
function addNoise(ctx, sigma, rng) {
  const W = ctx.canvas.width;
  const H = ctx.canvas.height;
  const img = ctx.getImageData(0, 0, W, H);
  const d = img.data;
  const driftR = (rng() * 2 - 1) * 18;
  const driftG = (rng() * 2 - 1) * 18;
  const driftB = (rng() * 2 - 1) * 18;
  for (let y = 0; y < H; y++) {
    const ty = y / H;
    for (let x = 0; x < W; x++) {
      const tx = x / W;
      const idx = (y * W + x) * 4;
      // Box–Muller
      const u1 = rng() || 1e-9;
      const u2 = rng();
      const mag = Math.sqrt(-2 * Math.log(u1)) * sigma;
      const a = 2 * Math.PI * u2;
      const drift = (tx + ty) * 0.5;
      d[idx] = clamp255(d[idx] + mag * Math.cos(a) + driftR * drift);
      d[idx + 1] = clamp255(d[idx + 1] + mag * Math.sin(a) + driftG * drift);
      d[idx + 2] = clamp255(d[idx + 2] + (rng() * 2 - 1) * sigma * 0.7 + driftB * drift);
    }
  }
  ctx.putImageData(img, 0, 0);
}
function clamp255(v) {
  return Math.max(0, Math.min(255, v | 0));
}

function addSpeckles(ctx, count, rng) {
  const W = ctx.canvas.width, H = ctx.canvas.height;
  for (let i = 0; i < count; i++) {
    ctx.strokeStyle = `rgba(${(rng() * 255) | 0},${(rng() * 255) | 0},${(rng() * 255) | 0},${0.15 + rng() * 0.25})`;
    ctx.lineWidth = 0.5 + rng() * 1.2;
    ctx.beginPath();
    const x = rng() * W, y = rng() * H, len = 1 + rng() * 4, ang = rng() * Math.PI * 2;
    ctx.moveTo(x, y);
    ctx.lineTo(x + Math.cos(ang) * len, y + Math.sin(ang) * len);
    ctx.stroke();
  }
}

function drawPerson(ctx, rng) {
  const W = ctx.canvas.width, H = ctx.canvas.height;
  // Background — uniform colour with mild variation
  const bg = 80 + rng() * 80;
  ctx.fillStyle = `rgb(${bg | 0},${(bg - 10) | 0},${(bg - 20) | 0})`;
  ctx.fillRect(0, 0, W, H);
  // Body
  const skin = [180 + rng() * 40, 140 + rng() * 30, 100 + rng() * 30];
  const shirt = [40 + rng() * 150, 40 + rng() * 150, 40 + rng() * 150];
  const cx = W * 0.5;
  const headR = 14 + rng() * 4;
  const bodyW = 38 + rng() * 8;
  const bodyH = 52 + rng() * 8;
  const headY = H * 0.34;
  const bodyY = headY + headR + 2;
  // Body rectangle with vertical shading
  const grd = ctx.createLinearGradient(cx - bodyW / 2, 0, cx + bodyW / 2, 0);
  grd.addColorStop(0, `rgb(${shirt[0] * 0.7 | 0},${shirt[1] * 0.7 | 0},${shirt[2] * 0.7 | 0})`);
  grd.addColorStop(1, `rgb(${shirt[0] | 0},${shirt[1] | 0},${shirt[2] | 0})`);
  ctx.fillStyle = grd;
  ctx.fillRect(cx - bodyW / 2, bodyY, bodyW, bodyH);
  // Arms
  ctx.fillRect(cx - bodyW / 2 - 8, bodyY + 2, 8, bodyH - 14);
  ctx.fillRect(cx + bodyW / 2, bodyY + 2, 8, bodyH - 14);
  // Head (ellipse-ish via circle)
  ctx.fillStyle = `rgb(${skin[0] | 0},${skin[1] | 0},${skin[2] | 0})`;
  ctx.beginPath();
  ctx.ellipse(cx, headY, headR * 0.85, headR, 0, 0, Math.PI * 2);
  ctx.fill();
  // Texture
  addSpeckles(ctx, 120, rng);
  addNoise(ctx, 14, rng);
}

function drawScene(ctx, rng) {
  const W = ctx.canvas.width, H = ctx.canvas.height;
  const variant = rng() < 0.5 ? "outdoor" : "indoor";
  if (variant === "outdoor") {
    const skyH = H * (0.35 + rng() * 0.2);
    ctx.fillStyle = `rgb(${130 + rng() * 60 | 0},${160 + rng() * 40 | 0},${210 + rng() * 30 | 0})`;
    ctx.fillRect(0, 0, W, skyH);
    ctx.fillStyle = `rgb(${60 + rng() * 40 | 0},${100 + rng() * 40 | 0},${50 + rng() * 30 | 0})`;
    ctx.fillRect(0, skyH, W, H - skyH);
    // A couple of "tree" blobs
    for (let i = 0; i < 3 + (rng() * 3) | 0; i++) {
      ctx.fillStyle = `rgb(${30 + rng() * 50 | 0},${80 + rng() * 40 | 0},${30 + rng() * 30 | 0})`;
      const tx = rng() * W;
      const tw = 10 + rng() * 20;
      const th = 15 + rng() * 30;
      ctx.fillRect(tx, skyH - th, tw, th);
    }
  } else {
    const wallH = H * (0.5 + rng() * 0.2);
    ctx.fillStyle = `rgb(${180 + rng() * 40 | 0},${170 + rng() * 40 | 0},${150 + rng() * 40 | 0})`;
    ctx.fillRect(0, 0, W, wallH);
    ctx.fillStyle = `rgb(${70 + rng() * 40 | 0},${50 + rng() * 30 | 0},${30 + rng() * 20 | 0})`;
    ctx.fillRect(0, wallH, W, H - wallH);
    ctx.fillStyle = `rgba(0,0,0,0.4)`;
    ctx.fillRect(0, wallH - 5, W, 10);
    // Some box outlines
    for (let i = 0; i < 2 + (rng() * 2) | 0; i++) {
      const ox = rng() * (W - 30);
      const ow = 14 + rng() * 22;
      const oh = 12 + rng() * 22;
      ctx.fillStyle = `rgba(${50 + rng() * 80 | 0},${30 + rng() * 60 | 0},${20 + rng() * 40 | 0},0.7)`;
      ctx.fillRect(ox, wallH - oh, ow, oh);
    }
  }
  addSpeckles(ctx, 120, rng);
  addNoise(ctx, 14, rng);
}

// ── Additional binary-task generators ──────────────────────────────────────
// Each generator draws into a 128x128 canvas in-place. The post-pass
// addNoise + addSpeckles ensures the feature extractor sees high-frequency
// content (MCUNet was pretrained on photographs; flat shapes alone collapse
// to identical layer-50 activations — see webapp/js/app.js comment block).

// Circle (class A) vs square (class B). Each filled with a randomised
// colour on a neutral background. Tests shape-detection capability.
function drawCircle(ctx, rng) {
  const W = ctx.canvas.width, H = ctx.canvas.height;
  const bg = 100 + rng() * 80;
  ctx.fillStyle = `rgb(${bg | 0},${(bg - 5) | 0},${(bg - 10) | 0})`;
  ctx.fillRect(0, 0, W, H);
  const r = 28 + rng() * 12;
  const cx = W * 0.5 + (rng() * 20 - 10);
  const cy = H * 0.5 + (rng() * 20 - 10);
  ctx.fillStyle = `rgb(${60 + rng() * 180 | 0},${60 + rng() * 180 | 0},${60 + rng() * 180 | 0})`;
  ctx.beginPath();
  ctx.arc(cx, cy, r, 0, Math.PI * 2);
  ctx.fill();
  addSpeckles(ctx, 120, rng);
  addNoise(ctx, 14, rng);
}
function drawSquare(ctx, rng) {
  const W = ctx.canvas.width, H = ctx.canvas.height;
  const bg = 100 + rng() * 80;
  ctx.fillStyle = `rgb(${bg | 0},${(bg - 5) | 0},${(bg - 10) | 0})`;
  ctx.fillRect(0, 0, W, H);
  const s = 50 + rng() * 24;
  const cx = W * 0.5 + (rng() * 20 - 10);
  const cy = H * 0.5 + (rng() * 20 - 10);
  ctx.fillStyle = `rgb(${60 + rng() * 180 | 0},${60 + rng() * 180 | 0},${60 + rng() * 180 | 0})`;
  ctx.fillRect(cx - s / 2, cy - s / 2, s, s);
  addSpeckles(ctx, 120, rng);
  addNoise(ctx, 14, rng);
}

// Vertical (class A) vs horizontal (class B) stripes. Same colour palette,
// only orientation differs. Tests orientation-sensitivity of features.
function drawStripes(ctx, rng, vertical) {
  const W = ctx.canvas.width, H = ctx.canvas.height;
  const stripeWidth = 8 + (rng() * 8) | 0;
  const c1 = [60 + rng() * 100, 80 + rng() * 100, 60 + rng() * 100];
  const c2 = [150 + rng() * 100, 130 + rng() * 100, 150 + rng() * 100];
  for (let p = 0; p < (vertical ? W : H); p += stripeWidth) {
    ctx.fillStyle = (Math.floor(p / stripeWidth) & 1)
      ? `rgb(${c1[0] | 0},${c1[1] | 0},${c1[2] | 0})`
      : `rgb(${c2[0] | 0},${c2[1] | 0},${c2[2] | 0})`;
    if (vertical) ctx.fillRect(p, 0, stripeWidth, H);
    else ctx.fillRect(0, p, W, stripeWidth);
  }
  addSpeckles(ctx, 120, rng);
  addNoise(ctx, 14, rng);
}

// Registry: every dataset has a key, display name, two class labels, and
// two draw functions (one per label). Picked by the #cfg-dataset <select>
// in index.html.
const DATASETS = {
  "person-vs-scene": {
    name: "person vs scene",
    labels: ["person", "scene"],
    drawers: [drawPerson, drawScene],
  },
  "circle-vs-square": {
    name: "circle vs square",
    labels: ["circle", "square"],
    drawers: [drawCircle, drawSquare],
  },
  "vstripes-vs-hstripes": {
    name: "vertical vs horizontal stripes",
    labels: ["vstripes", "hstripes"],
    drawers: [
      (ctx, rng) => drawStripes(ctx, rng, true),
      (ctx, rng) => drawStripes(ctx, rng, false),
    ],
  },
};

function cloneCanvas(source) {
  const c = document.createElement("canvas");
  c.width = IMG_W;
  c.height = IMG_H;
  c.getContext("2d").drawImage(source, 0, 0, IMG_W, IMG_H);
  return c;
}

function buildDataset(seed, datasetKey, synthTrainPerClass, synthTestPerClass) {
  const def = DATASETS[datasetKey] || DATASETS["person-vs-scene"];
  setClassNames(def.labels[0], def.labels[1]);
  const rng = makeRng(seed);
  const items = [];
  const mk = (drawerIdx, label, isTest, source = "synthetic", name = null) => {
    const c = document.createElement("canvas");
    c.width = IMG_W;
    c.height = IMG_H;
    def.drawers[drawerIdx](c.getContext("2d"), rng);
    items.push({ canvas: c, label, isTest, source, name, lastPred: null });
  };
  // Synthetic train samples — A/B interleaved.
  for (let i = 0; i < synthTrainPerClass; i++) {
    mk(0, PERSON, false);
    mk(1, SCENE, false);
  }
  for (const label of [PERSON, SCENE]) {
    for (const uploaded of S.uploads.train[label]) {
      items.push({
        canvas: cloneCanvas(uploaded.canvas),
        label,
        isTest: false,
        source: "file",
        name: uploaded.name,
        lastPred: null,
      });
    }
  }
  // Synthetic held-out validation/test samples — drawn from the same
  // generator + RNG stream so it
  // shares the same distribution but consists of samples the model never
  // sees during training. invoke_inf is run on them once per epoch; no
  // gradient ever feeds back.
  for (let i = 0; i < synthTestPerClass; i++) {
    mk(0, PERSON, true);
    mk(1, SCENE, true);
  }
  for (const label of [PERSON, SCENE]) {
    for (const uploaded of S.uploads.test[label]) {
      items.push({
        canvas: cloneCanvas(uploaded.canvas),
        label,
        isTest: true,
        source: "file",
        name: uploaded.name,
        lastPred: null,
      });
    }
  }
  return items;
}

function renderGallery() {
  galleryEl.innerHTML = "";
  S.dataset.forEach((item, idx) => {
    const tile = document.createElement("div");
    tile.className = "tile " + (item.lastPred == null
      ? "untouched"
      : (item.lastPred === item.label ? "correct" : "wrong"));
    // Test tiles get a yellow corner badge to make the train/test split
    // visible at a glance.
    if (item.isTest) tile.style.outline = "2px dashed #d4b94a";
    const display = document.createElement("canvas");
    display.width = 96;
    display.height = 96;
    display.getContext("2d").drawImage(item.canvas, 0, 0, 96, 96);
    tile.appendChild(display);
    const lbl = document.createElement("div");
    lbl.className = "lbl";
    const src = item.source === "file" ? "*" : "";
    let txt = `${item.isTest ? "V" : "T"}${idx}${src}: ${CLASS_NAMES[item.label]}`;
    if (item.lastPred != null) {
      txt += ` → ${CLASS_NAMES[item.lastPred]}`;
      if (item.lastLogitDelta != null) {
        txt += ` (Δ=${item.lastLogitDelta >= 0 ? "+" : ""}${item.lastLogitDelta})`;
      }
      if (
        item.lastFullArgmax != null && item.lastFullArgmax !== PERSON
        && item.lastFullArgmax !== SCENE
      ) {
        txt += `, raw=${item.lastFullArgmax}`;
      }
    }
    lbl.textContent = txt;
    tile.appendChild(lbl);
    galleryEl.appendChild(tile);
  });
}

// ── Curve rendering ────────────────────────────────────────────────────────
function drawChartAxes(ctx, W, H, yMax, yFormat, epochCount) {
  ctx.strokeStyle = "#444";
  ctx.beginPath();
  ctx.moveTo(28, 4);
  ctx.lineTo(28, H - 18);
  ctx.lineTo(W - 4, H - 18);
  ctx.stroke();
  ctx.fillStyle = "#888";
  ctx.font = "10px ui-monospace, monospace";

  for (const frac of [0, 0.5, 1]) {
    const y = (H - 18) - frac * (H - 22);
    ctx.fillText(yFormat(yMax * frac), 0, y + 3);
    if (frac > 0 && frac < 1) {
      ctx.strokeStyle = "#2a2a2a";
      ctx.beginPath();
      ctx.moveTo(28, y);
      ctx.lineTo(W - 4, y);
      ctx.stroke();
    }
  }
  if (epochCount > 0) {
    ctx.fillText("1", 28, H - 4);
    ctx.fillText(String(epochCount), W - 52, H - 4);
  }
}

function drawLogChartAxes(ctx, W, H, minVal, maxVal, yFormat, epochCount) {
  ctx.strokeStyle = "#444";
  ctx.beginPath();
  ctx.moveTo(28, 4);
  ctx.lineTo(28, H - 18);
  ctx.lineTo(W - 4, H - 18);
  ctx.stroke();
  ctx.fillStyle = "#888";
  ctx.font = "10px ui-monospace, monospace";
  const logMin = Math.log10(minVal);
  const logMax = Math.log10(maxVal);
  for (const frac of [0, 0.5, 1]) {
    const y = (H - 18) - frac * (H - 22);
    const v = 10 ** (logMin + frac * (logMax - logMin));
    ctx.fillText(yFormat(v), 0, y + 3);
    if (frac > 0 && frac < 1) {
      ctx.strokeStyle = "#2a2a2a";
      ctx.beginPath();
      ctx.moveTo(28, y);
      ctx.lineTo(W - 4, y);
      ctx.stroke();
    }
  }
  if (epochCount > 0) {
    ctx.fillText("1", 28, H - 4);
    ctx.fillText(String(epochCount), W - 52, H - 4);
  }
}

function drawLossCurve() {
  const c = $("loss-curve");
  const ctx = c.getContext("2d");
  const W = c.width, H = c.height;
  const scale = $("loss-scale") ? $("loss-scale").value : "linear";
  const positiveLosses = S.trainLosses.filter((v) => Number.isFinite(v) && v > 0);
  const maxL = Math.max(2.5, ...S.trainLosses);
  const minLogL = Math.max(1e-4, Math.min(...positiveLosses, 0.1));
  const maxLogL = Math.max(minLogL * 10, ...positiveLosses, 2.5);
  ctx.clearRect(0, 0, W, H);
  if (scale === "log") {
    drawLogChartAxes(
      ctx,
      W,
      H,
      minLogL,
      maxLogL,
      (v) => v >= 1 ? v.toFixed(1) : v.toPrecision(1),
      S.trainLosses.length,
    );
  } else {
    drawChartAxes(ctx, W, H, maxL, (v) => v.toFixed(1), S.trainLosses.length);
  }
  ctx.fillStyle = "#888";
  ctx.font = "10px ui-monospace, monospace";
  ctx.fillText("avg loss", 0, 12);
  ctx.fillText("epoch", W - 32, H - 4);
  if (S.trainLosses.length < 2) return;
  const n = S.trainLosses.length;
  ctx.strokeStyle = "#6fc06f";
  ctx.lineWidth = 1.5;
  ctx.beginPath();
  for (let i = 0; i < n; i++) {
    const x = 28 + (i / Math.max(1, n - 1)) * (W - 32);
    const frac = scale === "log"
      ? (Math.log10(Math.max(minLogL, S.trainLosses[i])) - Math.log10(minLogL))
        / (Math.log10(maxLogL) - Math.log10(minLogL))
      : S.trainLosses[i] / maxL;
    const y = (H - 18) - frac * (H - 22);
    if (i === 0) ctx.moveTo(x, y);
    else ctx.lineTo(x, y);
  }
  ctx.stroke();
}

function drawSramCurve() {
  const c = $("sram-curve");
  const ctx = c.getContext("2d");
  const W = c.width, H = c.height;
  const maxSram = Math.max(1, S.sramPeakOverall, ...S.arenaEpochAvgs);
  ctx.clearRect(0, 0, W, H);
  drawChartAxes(ctx, W, H, maxSram, (v) => (v / 1024).toFixed(0), S.arenaEpochAvgs.length);
  ctx.fillStyle = "#888";
  ctx.font = "10px ui-monospace, monospace";
  ctx.fillText("arena KB", 0, 12);
  ctx.fillText("epoch", W - 32, H - 4);
  if (S.arenaEpochAvgs.length < 1) return;
  const n = S.arenaEpochAvgs.length;
  ctx.strokeStyle = "#d29922";
  ctx.lineWidth = 1.5;
  ctx.beginPath();
  for (let i = 0; i < n; i++) {
    const x = 28 + (i / Math.max(1, n - 1)) * (W - 32);
    const y = (H - 18) - (S.arenaEpochAvgs[i] / maxSram) * (H - 22);
    if (i === 0) ctx.moveTo(x, y);
    else ctx.lineTo(x, y);
  }
  ctx.stroke();
  if (S.sramPeakOverall > 0) {
    ctx.fillStyle = "#d29922";
    ctx.fillText(`peak ${fmtKb(S.sramPeakOverall)}`, 118, 12);
  }
}

// ── Image bytes → blob URL helper ──────────────────────────────────────────
// The wasm expects 80*80*3 = 19200 signed-int8 bytes (R−128, G−128, B−128).
function canvasToBlobUrl(canvas) {
  const ctx = canvas.getContext("2d");
  const pixels = ctx.getImageData(0, 0, IMG_W, IMG_H).data;
  const buf = new Int8Array(IMG_W * IMG_H * 3);
  let j = 0;
  for (let i = 0; i < IMG_W * IMG_H; i++) {
    buf[j++] = pixels[i * 4 + 0] - 128;
    buf[j++] = pixels[i * 4 + 1] - 128;
    buf[j++] = pixels[i * 4 + 2] - 128;
  }
  return URL.createObjectURL(new Blob([buf], { type: "application/octet-stream" }));
}

// ── Wasm reply plumbing ────────────────────────────────────────────────────
// We use a single in-flight slot: each step awaits the corresponding
// {type:"infer_result"|"train_result"} reply before issuing the next step.
function handleReply(payload, parsed) {
  if (!parsed || !S.pendingReply) return;
  if (
    parsed.type === "infer_result" || parsed.type === "train_result"
    || parsed.type === "bias42" || parsed.type === "reset_ack"
    || parsed.type === "set_lr_ack" || parsed.type === "input_sig"
    || parsed.type === "pooled_sig"
    || parsed.type === "memory"
    || parsed.type === "train_debug" || parsed.type === "error"
  ) {
    const resolver = S.pendingReply;
    S.pendingReply = null;
    resolver(parsed);
  }
}

// Sends a set_lr envelope to the wasm and waits for the ack. Called both
// when the user changes the LR/BLR inputs and at the start of every
// training run, so the wasm's lr/blr stay in sync with the UI.
async function pushLrFromUi() {
  if (!S.controller) return null;
  const lr = parseFloat($("cfg-lr").value);
  const blr = parseFloat($("cfg-blr").value);
  const binary_lr = parseFloat($("cfg-binary-lr").value);
  if (!isFinite(lr) || !isFinite(blr) || !isFinite(binary_lr)) return null;
  const reply = await submitAndWait({ type: "set_lr", lr, blr, binary_lr }, null);
  if (reply && reply.type === "set_lr_ack") {
    // Default training uses the frozen generated features plus a stable
    // fp32 prototype head. The LR field is retained for old envelopes.
    log(
      `  prototype_lr=${reply.binary_lr.toExponential(2)} (on-graph LR=${(0.0008).toExponential(2)} BLR=${
        (0.0004).toExponential(2)
      })`,
    );
    return reply;
  }
  return null;
}

async function awaitReply() {
  return new Promise((resolve) => {
    S.pendingReply = resolve;
  });
}

async function submitAndWait(envelope, urlToRevoke) {
  S.controller.submit(envelope);
  const reply = await awaitReply();
  if (urlToRevoke) URL.revokeObjectURL(urlToRevoke);
  return reply;
}

// ── Training + evaluation loops ────────────────────────────────────────────
// Held-out test evaluation. Runs invoke_inf on every test sample (no
// gradient, no weight update) and reports binary-projection accuracy.
// Called once per epoch from runTraining(). The test items were drawn
// from the same RNG stream as the training items but never feed
// invoke(labels), so this is true generalisation accuracy.
async function evaluateTest() {
  const perClassCorrect = [0, 0];
  const perClassTotal = [0, 0];
  let predCount0 = 0, predCount1 = 0;
  let testIdx = 0;
  const totalTest = S.dataset.filter((item) => item.isTest).length;
  log(`  — held-out test eval —`);
  for (let i = 0; i < S.dataset.length; i++) {
    if (S.stop) break;
    const item = S.dataset[i];
    if (!item.isTest) continue;
    const url = canvasToBlobUrl(item.canvas);
    const reply = await submitAndWait({ type: "infer", url }, url);
    if (reply.type === "error") {
      log(`! test infer error at sample ${i}: ${reply.message}`);
      continue;
    }
    const lp = scoreAt(reply, PERSON);
    const ls = scoreAt(reply, SCENE);
    const binaryPred = lp >= ls ? PERSON : SCENE;
    item.lastPred = binaryPred;
    item.lastLogitDelta = lp - ls;
    item.lastFullArgmax = reply.full_argmax ?? reply.argmax;
    perClassTotal[item.label]++;
    const correct = binaryPred === item.label;
    if (correct) perClassCorrect[item.label]++;
    if (binaryPred === PERSON) predCount0++;
    else predCount1++;
    // Per-test-sample log line — same format as the training-step lines so
    // you can visually scan ✓/✗ across the test split each epoch.
    testIdx++;
    log(
      `    v${testIdx.toString().padStart(2)}/${totalTest}`
        + ` true=${CLASS_NAMES[item.label].padEnd(8)}`
        + ` pred=${CLASS_NAMES[binaryPred].padEnd(8)}`
        + ` ${correct ? "✓" : "✗"}`
        + ` Δ=${fmtDelta(lp - ls)}`,
    );
  }
  renderGallery();
  const accPerson = perClassTotal[PERSON] ? perClassCorrect[PERSON] / perClassTotal[PERSON] : 0;
  const accScene = perClassTotal[SCENE] ? perClassCorrect[SCENE] / perClassTotal[SCENE] : 0;
  const totalDen = perClassTotal[0] + perClassTotal[1];
  const accOverall = totalDen ? (perClassCorrect[0] + perClassCorrect[1]) / totalDen : 0;
  return {
    accPerson,
    accScene,
    accOverall,
    predCount0,
    predCount1,
    correctPerson: perClassCorrect[PERSON],
    totalPerson: perClassTotal[PERSON],
    correctScene: perClassCorrect[SCENE],
    totalScene: perClassTotal[SCENE],
  };
}

async function trainEpoch() {
  // Iterate only training samples. Validation/test samples are never fed to
  // invoke — see evaluateTest() below for the held-out pass.
  const trainIdx = S.dataset
    .map((item, i) => (item.isTest ? -1 : i))
    .filter((i) => i >= 0);
  const order = balancedTrainOrder(trainIdx);
  log(`  order: ${orderSummary(order)}`);
  let epochLoss = 0;
  let n = 0;
  let epochCorrect = 0;
  let epochArenaTotal = 0;
  let epochArenaSamples = 0;
  for (let i = 0; i < order.length; i++) {
    if (S.stop) break;
    const item = S.dataset[order[i]];

    // ── Step 1: PRE-UPDATE inference ──────────────────────────────────────
    // Ask the model what it predicts BEFORE we adjust weights for this
    // sample. This is the honest training accuracy — it reflects how well
    // the model classifies each sample given the cumulative state of all
    // prior training, NOT given a fresh single-sample overfit. With this
    // ordering the metric starts low (~50% with our balanced binary task
    // and constant initial bias) and climbs as the model genuinely learns
    // across samples.
    const inferUrl = canvasToBlobUrl(item.canvas);
    const inferReply = await submitAndWait({ type: "infer", url: inferUrl }, inferUrl);
    if (inferReply.type !== "infer_result") {
      log(`! pre-update infer error at sample ${order[i]}: ${inferReply.message ?? "unknown"}`);
      continue;
    }
    const lp = scoreAt(inferReply, PERSON);
    const ls = scoreAt(inferReply, SCENE);
    // The task loss is binary: only PERSON and SCENE from scoreAt() enter
    // the softmax.
    const probArr = [0, 0];
    probArr[PERSON] = lp / 8;
    probArr[SCENE] = ls / 8;
    const binaryProbs = softmax(probArr);
    const loss = -Math.log(Math.max(1e-7, binaryProbs[item.label]));
    const binaryPred = lp >= ls ? PERSON : SCENE;
    const correct = binaryPred === item.label;
    epochLoss += loss;
    n++;
    if (correct) {
      epochCorrect++;
      S.runningCorrect++;
    }
    S.runningTotal++;
    setMetric("m-step", `${i + 1}/${order.length}`);
    setMetric("m-loss", loss.toFixed(3));
    // Live training accuracy = running pre-update ✓ count / total steps
    // seen so far. This climbs as the model actually learns the boundary,
    // rather than being pinned at 100% by the post-update measurement.
    // We also carry forward the LAST validation accuracy (from the
    // previous epoch's evaluateTest) so the held-out number stays visible
    // throughout the current epoch instead of only appearing momentarily.
    const trainPct = S.runningTotal
      ? (S.runningCorrect / S.runningTotal * 100).toFixed(1)
      : null;
    const valPct = S.lastTestAcc != null
      ? (S.lastTestAcc * 100).toFixed(1)
      : null;
    setMetric(
      "m-acc",
      (trainPct != null ? `t ${trainPct}%` : "t —")
        + " / "
        + (valPct != null ? `v ${valPct}%` : "v —"),
    );
    updateStepDisplay(item, scoreVector(inferReply), loss);
    log(
      `  ep${S.epoch + 1} s${(i + 1).toString().padStart(2)}/${order.length}`
        + ` true=${CLASS_NAMES[item.label].padEnd(8)}`
        + ` pred=${CLASS_NAMES[binaryPred].padEnd(8)}`
        + ` loss=${loss.toFixed(3)} ${correct ? "✓" : "✗"}`
        + ` Δ=${fmtDelta(lp - ls)}`,
    );

    // ── Step 2: training update ──────────────────────────────────────────
    // Now apply the gradient. We send the same image bytes; the wasm
    // re-fetches them and runs invoke(labels) which does forward + backward
    // + update. We don't need the reply (other than confirming success)
    // because we already have the pre-update logits.
    const url = canvasToBlobUrl(item.canvas);
    const trainReply = await submitAndWait(
      { type: "train", url, label: item.label },
      url,
    );
    if (trainReply.type === "error") {
      log(`! train error at sample ${order[i]}: ${trainReply.message}`);
    } else if (Number.isFinite(trainReply.sram_current)) {
      S.sramPeakOverall = Math.max(S.sramPeakOverall, trainReply.sram_peak ?? trainReply.sram_current);
      setMetric("m-sram", fmtKb(S.sramPeakOverall));
      if (Number.isFinite(trainReply.arena_touched)) {
        epochArenaTotal += trainReply.arena_touched;
        epochArenaSamples++;
        S.arenaPeakOverall = Math.max(S.arenaPeakOverall, trainReply.arena_touched);
        setMetric("m-arena", fmtKb(S.arenaPeakOverall));
      }
    }
  }
  return {
    avgLoss: n > 0 ? epochLoss / n : NaN,
    epochCorrect,
    epochTotal: order.length,
    avgArena: epochArenaSamples > 0 ? epochArenaTotal / epochArenaSamples : NaN,
  };
}

function softmax(arr) {
  const m = Math.max(...arr);
  const e = arr.map((v) => Math.exp(v - m));
  const s = e.reduce((a, b) => a + b, 0);
  return e.map((v) => v / s);
}

function shuffleInPlace(arr) {
  for (let i = arr.length - 1; i > 0; i--) {
    const j = Math.floor(Math.random() * (i + 1));
    [arr[i], arr[j]] = [arr[j], arr[i]];
  }
  return arr;
}

// Paper-faithful (arXiv:2206.15472): random shuffle per epoch — the
// reference implementation, the tinyengine tutorial, and the original
// webapp all use a plain shuffled order. Strict class alternation
// (max_run=1) is the worst-case schedule for single-sample SGD because
// every update is immediately undone by an opposite-class update,
// producing the ±15-30 unit Δ oscillation seen in the logs. A full
// shuffle lets like-class runs occur naturally and lets the head bias
// settle on whichever class the recent gradient mass points toward.
function balancedTrainOrder(trainIdx) {
  return shuffleInPlace(trainIdx.slice());
}

function orderSummary(order) {
  let p = 0, s = 0, maxRun = 0, run = 0, prev = -1;
  for (const idx of order) {
    const label = S.dataset[idx].label;
    if (label === PERSON) p++;
    if (label === SCENE) s++;
    if (label === prev) run++;
    else run = 1;
    prev = label;
    if (run > maxRun) maxRun = run;
  }
  return `${CLASS_NAMES[PERSON]}=${p}, ${CLASS_NAMES[SCENE]}=${s}, max_run=${maxRun}`;
}

// Live per-step display — ported from webapp/js/app.js:updateCurrentImage +
// updateStepMetrics. Draws the just-trained sample with a green border if
// the model now classifies it correctly (logits[0] vs logits[1]) or red if
// not, plus the live loss and a horizontal bar per class showing softmax
// probability. Bars use temperature 8 so the values don't peg at 0/1 with
// the int8 logits' wide [-128, 127] range.
function updateStepDisplay(item, logits, loss) {
  const stepCanvas = document.getElementById("step-canvas");
  if (!stepCanvas) return;
  const ctx = stepCanvas.getContext("2d");
  ctx.drawImage(item.canvas, 0, 0, stepCanvas.width, stepCanvas.height);

  const lp = logits[PERSON];
  const ls = logits[SCENE];
  const binaryPred = lp >= ls ? PERSON : SCENE;
  const correct = binaryPred === item.label;
  ctx.strokeStyle = correct ? "#3fb950" : "#f85149";
  ctx.lineWidth = 6;
  ctx.strokeRect(3, 3, stepCanvas.width - 6, stepCanvas.height - 6);

  const stepTrue = document.getElementById("step-true");
  const stepPred = document.getElementById("step-pred");
  const stepLoss = document.getElementById("step-loss");
  stepTrue.textContent = CLASS_NAMES[item.label];
  stepTrue.style.color = CLASS_COLORS[item.label];
  stepPred.textContent = CLASS_NAMES[binaryPred];
  stepPred.style.color = CLASS_COLORS[binaryPred];
  stepLoss.textContent = loss.toFixed(3);

  // Render one bar per class. Temperature-8 softmax to match the demo's
  // existing loss-curve scaling.
  const probs = softmax(logits.map((l) => l / 8));
  const bars = document.getElementById("pred-bars");
  bars.innerHTML = "";
  for (let i = 0; i < probs.length; i++) {
    const pct = (probs[i] * 100).toFixed(1);
    const row = document.createElement("div");
    row.className = "bar-row";
    const lbl = document.createElement("span");
    lbl.className = "bar-label";
    lbl.textContent = CLASS_NAMES[i];
    if (i === item.label) lbl.style.fontWeight = "700"; // bold the true class
    const trk = document.createElement("div");
    trk.className = "bar-track";
    const fill = document.createElement("div");
    fill.className = "bar-fill";
    fill.style.width = `${Math.max(1, probs[i] * 100)}%`;
    fill.style.background = CLASS_COLORS[i];
    fill.style.opacity = i === item.label ? 1.0 : (i === PERSON || i === SCENE ? 0.85 : 0.45);
    trk.appendChild(fill);
    const pctEl = document.createElement("span");
    pctEl.className = "bar-pct";
    pctEl.textContent = `${pct}%`;
    row.append(lbl, trk, pctEl);
    bars.appendChild(row);
  }
}

async function readBias42() {
  // No-op: older demo code inspected a bias42 tensor that is not part of
  // this generated graph surface. Kept so existing callers don't crash.
  return null;
}

async function readTrainDebug() {
  const reply = await submitAndWait({ type: "get_train_debug" }, null);
  return reply && reply.type === "train_debug" ? reply : null;
}

function fmtTrainDebug(s) {
  if (!s) return "unavailable";
  return `changed=${s.total_changed} abs=${s.total_abs} hash=${s.all_hash}`
    + ` | head_w=${s.head_w_changed}/${s.head_w_abs}`
    + ` head_b=${s.head_b_changed}/${s.head_b_abs}`
    + ` block_w=${s.block_w_changed}/${s.block_w_abs}`
    + ` block_b=${s.block_b_changed}/${s.block_b_abs}`
    + ` binary=${s.binary_changed ?? 0}/${s.binary_abs ?? 0}`
    + ` updates=${s.binary_updates ?? 0}`;
}

function logTrainDebug(prefix, s) {
  log(`  ${prefix}: ${fmtTrainDebug(s)}`);
  if (s) {
    log(
      `    hashes all=${s.all_hash} head_w=${s.head_w_hash} head_b=${s.head_b_hash}`
        + ` block_w=${s.block_w_hash} block_b=${s.block_b_hash}`
        + ` snapshot=${s.snapshot_ready}`,
    );
  }
}

function scoreAt(reply, idx) {
  let s = 0;
  if (reply.scores) s += reply.scores[idx] / 1024;
  else if (reply.logits) s += reply.logits[idx];
  if (reply.binary_scores && idx < BINARY_CLASSES) s += reply.binary_scores[idx] / 1024;
  return s;
}

function scoreVector(reply) {
  return [scoreAt(reply, SCENE), scoreAt(reply, PERSON)];
}

function logitDelta(reply) {
  return scoreAt(reply, PERSON) - scoreAt(reply, SCENE);
}

function binaryFromReply(reply) {
  return logitDelta(reply) >= 0 ? PERSON : SCENE;
}

// Raw built-in-head scores only — used by the score-separability
// verifier which wants to characterise the frozen backbone's response,
// not the binary head's adaptation on top.
function rawScoreVector(reply) {
  return reply.scores ? reply.scores.map((v) => v / 1024) : reply.logits;
}

function fmtDelta(v) {
  const rounded = Math.round(v * 10) / 10;
  return `${rounded >= 0 ? "+" : ""}${rounded}`;
}

async function collectScoreMeans(items, title) {
  const sums = [Array(NUM_CLASSES).fill(0), Array(NUM_CLASSES).fill(0)];
  const counts = [0, 0];
  for (const item of items) {
    const reply = await inferCanvas(item.canvas);
    if (reply.type !== "infer_result") continue;
    const scores = rawScoreVector(reply);
    if (item.label !== PERSON && item.label !== SCENE) continue;
    counts[item.label]++;
    for (let i = 0; i < NUM_CLASSES; i++) sums[item.label][i] += scores[i];
  }
  if (!counts[PERSON] || !counts[SCENE]) {
    log(`  ${title}: not enough samples for both classes`);
    return null;
  }
  const means = sums.map((sum, cls) => sum.map((v) => v / counts[cls]));
  return { title, counts, means };
}

function logScoreSeparability(stats) {
  if (!stats) return;
  const diff = stats.means[PERSON].map((v, i) => v - stats.means[SCENE][i]);
  let maxChannel = 0;
  for (let i = 1; i < NUM_CLASSES; i++) {
    if (Math.abs(diff[i]) > Math.abs(diff[maxChannel])) maxChannel = i;
  }
  let best = { i: 0, j: 1, sep: -Infinity };
  for (let i = 0; i < NUM_CLASSES; i++) {
    for (let j = 0; j < NUM_CLASSES; j++) {
      if (i === j) continue;
      const sep = Math.abs(
        (stats.means[PERSON][i] - stats.means[PERSON][j])
          - (stats.means[SCENE][i] - stats.means[SCENE][j]),
      );
      if (sep > best.sep) best = { i, j, sep };
    }
  }
  const pairSep = Math.abs(
    (stats.means[PERSON][PERSON] - stats.means[PERSON][SCENE])
      - (stats.means[SCENE][PERSON] - stats.means[SCENE][SCENE]),
  );
  log(
    `  ${stats.title}: n=${stats.counts[PERSON]}/${stats.counts[SCENE]}`
      + ` meanΔ[0-1]=${fmtDelta(pairSep)}`
      + ` best_ch=c${maxChannel} ${fmtDelta(diff[maxChannel])}`
      + ` best_pair=c${best.i}-c${best.j} ${fmtDelta(best.sep)}`,
  );
  if (best.sep < 1) {
    log(`    ⚠ Split is not separable in this model's current ${NUM_CLASSES} output scores.`);
  }
}

async function verifyClassSeparability() {
  log(`── score separability verification ──`);
  const train = S.dataset.filter((item) => !item.isTest);
  const test = S.dataset.filter((item) => item.isTest);
  logScoreSeparability(await collectScoreMeans(train, "train split"));
  logScoreSeparability(await collectScoreMeans(test, "validation split"));
}

async function runTraining() {
  const maxEpochs = parseInt($("cfg-epochs").value, 10) || 20;
  const counts = datasetCounts();
  if (counts.train === 0) {
    log("! add at least one training image before starting");
    return;
  }
  S.stop = false;
  S.epoch = 0;
  S.trainLosses = [];
  S.epochAccs = [];
  S.arenaEpochAvgs = [];
  S.sramPeakOverall = 0;
  S.arenaPeakOverall = 0;
  // Reset the running training-accuracy counters at the start of every run
  // so the displayed accuracy reflects this run, not historical totals.
  S.runningCorrect = 0;
  S.runningTotal = 0;
  S.lastTestAcc = null;
  drawLossCurve();
  setMetric("m-epoch", "0");
  setMetric("m-step", "—");
  setMetric("m-acc", "—");
  setMetric("m-loss", "—");
  setMetric("m-sram", "—");
  setMetric("m-arena", "—");
  drawSramCurve();
  $("btn-train").disabled = true;
  $("btn-stop").disabled = false;
  $("btn-regenerate").disabled = true;

  log(`▶ training start: ${maxEpochs} epochs × ${counts.train} train (+ ${counts.test} validation/test)`);

  // Sync the wasm's lr/blr to whatever the UI inputs currently say. Runs
  // every training run so toggling the inputs after a Reset takes effect
  // on the next click of Start training.
  await pushLrFromUi();

  // Baseline bias snapshot — every subsequent epoch's deltas are computed
  // relative to this. Non-zero deltas on bias42[0] and bias42[1] are
  // evidence training is mutating the trained heads.
  const biasBase = await readBias42();
  if (biasBase) log(`  bias42 base = [${biasBase.join(",")}]`);
  let biasPrev = biasBase ? biasBase.slice() : null;

  for (S.epoch = 0; S.epoch < maxEpochs && !S.stop; S.epoch++) {
    setMetric("m-epoch", String(S.epoch + 1));
    const epoch = await trainEpoch();
    if (S.stop) break;
    const epochAcc = epoch.epochTotal > 0
      ? epoch.epochCorrect / epoch.epochTotal
      : 0;
    S.epochAccs.push(epochAcc);
    // Loss curve plots one point per epoch (the epoch's average pre-update
    // training loss) rather than per-step. Cleaner trend; reflects "how
    // wrong was the model on average across this epoch's training samples".
    S.trainLosses.push(epoch.avgLoss);
    if (Number.isFinite(epoch.avgArena)) S.arenaEpochAvgs.push(epoch.avgArena);
    drawLossCurve();
    drawSramCurve();

    // Bias-delta probe — direct evidence that training is moving the head.
    let biasLine = "";
    if (biasPrev) {
      const biasNow = await readBias42();
      if (biasNow) {
        const dStep = biasNow.map((v, i) => v - biasPrev[i]);
        const dCum = biasBase ? biasNow.map((v, i) => v - biasBase[i]) : null;
        biasLine = ` Δbias42[0,1]=${dStep[0]},${dStep[1]}`
          + (dCum ? ` cum=${dCum[0]},${dCum[1]}` : "");
        biasPrev = biasNow.slice();
      }
    }
    const trainDebug = await readTrainDebug();
    const binaryLine = trainDebug
      ? ` binary_head=${trainDebug.binary_changed ?? 0}/${trainDebug.binary_abs ?? 0}`
        + ` updates=${trainDebug.binary_updates ?? 0}`
      : "";

    // Held-out test pass — never feeds invoke(), so this is the true
    // generalisation signal. Reaches 100% only when the head has actually
    // learnt the class boundary, not when it can fit a single sample.
    const test = await evaluateTest();
    S.lastTestAcc = test.accOverall;
    setMetric(
      "m-acc",
      `t ${(epochAcc * 100).toFixed(1)}%`
        + ` / v ${(test.accOverall * 100).toFixed(1)}%`,
    );

    log(
      `  ep ${S.epoch + 1}/${maxEpochs}: loss=${epoch.avgLoss.toFixed(3)}`
        + ` train_acc=${(epochAcc * 100).toFixed(1)}% (${epoch.epochCorrect}/${epoch.epochTotal})`
        + ` test_acc=${(test.accOverall * 100).toFixed(1)}% (${test.correctPerson + test.correctScene}/${
          test.totalPerson + test.totalScene
        })`
        + (Number.isFinite(epoch.avgArena)
          ? ` avg_arena_touched=${fmtKb(epoch.avgArena)} peak_sram=${fmtKb(S.sramPeakOverall)}`
          : "")
        + (S.arenaPeakOverall > 0 ? ` arena_touched_peak=${fmtKb(S.arenaPeakOverall)}` : "")
        + ` ['${CLASS_NAMES[PERSON]}'=${test.correctPerson}/${test.totalPerson},`
        + ` '${CLASS_NAMES[SCENE]}'=${test.correctScene}/${test.totalScene}]`
        + biasLine
        + binaryLine,
    );
  }

  log(S.stop ? `⏹ stopped at epoch ${S.epoch}` : `✓ training complete`);
  $("btn-train").disabled = false;
  $("btn-stop").disabled = true;
  $("btn-regenerate").disabled = false;
}

// ── File upload + wire up ─────────────────────────────────────────────────
function drawImageCover(ctx, img) {
  const sw = img.naturalWidth || img.width;
  const sh = img.naturalHeight || img.height;
  const scale = Math.max(IMG_W / sw, IMG_H / sh);
  const dw = sw * scale;
  const dh = sh * scale;
  ctx.fillStyle = "#000";
  ctx.fillRect(0, 0, IMG_W, IMG_H);
  ctx.drawImage(img, (IMG_W - dw) / 2, (IMG_H - dh) / 2, dw, dh);
}

async function fileToCanvas(file) {
  const url = URL.createObjectURL(file);
  try {
    const img = new Image();
    img.decoding = "async";
    img.src = url;
    await img.decode();
    const c = document.createElement("canvas");
    c.width = IMG_W;
    c.height = IMG_H;
    drawImageCover(c.getContext("2d"), img);
    return { canvas: c, name: file.name };
  } finally {
    URL.revokeObjectURL(url);
  }
}

async function imageUrlToCanvas(url, name) {
  const img = new Image();
  img.decoding = "async";
  img.src = url;
  await img.decode();
  const c = document.createElement("canvas");
  c.width = IMG_W;
  c.height = IMG_H;
  drawImageCover(c.getContext("2d"), img);
  return { canvas: c, name };
}

async function loadSamplePack() {
  const button = $("btn-load-sample-pack");
  if (button) button.disabled = true;
  try {
    const response = await fetch(SAMPLE_PACK_MANIFEST_URL, { cache: "no-store" });
    if (!response.ok) {
      throw new Error(`sample pack manifest not found (${response.status})`);
    }
    const manifest = await response.json();
    const base = new URL(SAMPLE_PACK_MANIFEST_URL, window.location.href);
    const readEntries = async (split, label, entries) => {
      const converted = [];
      for (const entry of entries || []) {
        const path = typeof entry === "string" ? entry : entry.path;
        const name = typeof entry === "string" ? entry.split("/").pop() : (entry.name || entry.path.split("/").pop());
        const url = new URL(path, base).href;
        try {
          converted.push(await imageUrlToCanvas(url, name));
        } catch (e) {
          log(`! failed to load sample ${path}: ${e?.message ?? e}`);
        }
      }
      S.uploads[split][label] = converted;
    };

    const data = manifest.datasets?.["person-vs-scene"];
    if (!data) throw new Error("manifest does not contain person-vs-scene");

    await readEntries("train", PERSON, data.train?.person);
    await readEntries("train", SCENE, data.train?.scene);
    await readEntries("test", PERSON, data.test?.person);
    await readEntries("test", SCENE, data.test?.scene);

    const datasetSelect = $("cfg-dataset");
    if (datasetSelect) datasetSelect.value = "person-vs-scene";
    regenerate();
    const counts = datasetCounts();
    log(`+ loaded local sample pack (${counts.fileTrain} training + ${counts.fileTest} validation/test images)`);
  } catch (e) {
    log(`! sample pack unavailable: ${e?.message ?? e}`);
  } finally {
    if (button) button.disabled = false;
    updateDatasetSummary();
  }
}

async function readUploadInput(inputId, split, label) {
  const input = $(inputId);
  const files = input ? Array.from(input.files || []) : [];
  S.uploads[split][label] = [];
  if (files.length === 0) {
    regenerate();
    return;
  }
  const button = $("btn-regenerate");
  if (button) button.disabled = true;
  try {
    const converted = [];
    for (const file of files) {
      if (!file.type.startsWith("image/")) {
        log(`! skipped non-image file: ${file.name}`);
        continue;
      }
      try {
        converted.push(await fileToCanvas(file));
      } catch (e) {
        log(`! failed to read ${file.name}: ${e?.message ?? e}`);
      }
    }
    S.uploads[split][label] = converted;
    log(
      `+ loaded ${converted.length}/${files.length} ${
        split === "train" ? "training" : "validation/test"
      } upload(s) for ${CLASS_NAMES[label]}`,
    );
    regenerate();
  } finally {
    if (button) button.disabled = false;
    updateDatasetSummary();
  }
}

function regenerate() {
  const seed = parseInt($("cfg-seed").value, 10) || 42;
  const key = $("cfg-dataset") ? $("cfg-dataset").value : "person-vs-scene";
  const synthTrain = clampIntInput("cfg-synth-train", DEFAULT_SYNTH_TRAIN_PER_CLASS, 0, 500);
  const synthTest = clampIntInput("cfg-synth-test", DEFAULT_SYNTH_TEST_PER_CLASS, 0, 500);
  S.dataset = buildDataset(seed, key, synthTrain, synthTest);
  renderGallery();
  updateDatasetSummary();
  const counts = datasetCounts();
  log(
    `+ dataset regenerated (${
      DATASETS[key].name
    }, seed=${seed}, ${counts.total} images: ${counts.train} train + ${counts.test} validation/test)`,
  );
}

$("btn-regenerate").addEventListener("click", regenerate);
$("cfg-dataset").addEventListener("change", regenerate);
$("cfg-synth-train").addEventListener("change", regenerate);
$("cfg-synth-test").addEventListener("change", regenerate);
$("loss-scale").addEventListener("change", drawLossCurve);
$("btn-load-sample-pack").addEventListener("click", loadSamplePack);
$("upload-train-a").addEventListener("change", () => readUploadInput("upload-train-a", "train", PERSON));
$("upload-train-b").addEventListener("change", () => readUploadInput("upload-train-b", "train", SCENE));
$("upload-test-a").addEventListener("change", () => readUploadInput("upload-test-a", "test", PERSON));
$("upload-test-b").addEventListener("change", () => readUploadInput("upload-test-b", "test", SCENE));

async function inferCanvas(canvas) {
  const url = canvasToBlobUrl(canvas);
  return submitAndWait({ type: "infer", url }, url);
}

async function trainCanvas(canvas, label) {
  const url = canvasToBlobUrl(canvas);
  return submitAndWait({ type: "train", url, label }, url);
}

// Input-pipeline verification — answers two questions definitively:
//  (a) Is the wasm receiving distinct bytes for distinct inputs?
//  (b) Does the frozen backbone produce different layer-50 features for
//      different inputs (i.e. are the int8 logits we observe genuinely
//      input-dependent)?
// If after a reset two clearly-different images produce IDENTICAL logits
// across all 10 outputs, the input path or backbone has a bug. If they
// produce different logits, the feature pipeline is healthy and any
// training failures are about the training dynamics, not the data path.
async function verifyInput() {
  if (!S.controller) return;
  $("btn-verify").disabled = true;
  log(`── input-pipeline verification ──`);

  // Reset to factory weights so the inference is on the untrained model.
  const r = await submitAndWait({ type: "reset" }, null);
  if (!r || r.type !== "reset_ack") {
    log("! reset failed; aborting verify");
    $("btn-verify").disabled = false;
    return;
  }

  // Find two test samples with opposite labels. Test items are at the end
  // of the dataset (isTest=true). One PERSON + one SCENE = maximally
  // different inputs available in this dataset.
  const a = S.dataset.find((it) => it.isTest && it.label === PERSON);
  const b = S.dataset.find((it) => it.isTest && it.label === SCENE);
  if (!a || !b) {
    log("! couldn't find one of each class in test split");
    $("btn-verify").disabled = false;
    return;
  }

  // First, compare the raw bytes that JS hands to the wasm so we know
  // whether the *inputs themselves* are distinct.
  const bytesA = canvasToInt8Bytes(a.canvas);
  const bytesB = canvasToInt8Bytes(b.canvas);
  let differingBytes = 0;
  let maxAbsDiff = 0;
  for (let i = 0; i < bytesA.length; i++) {
    const d = bytesA[i] - bytesB[i];
    if (d !== 0) differingBytes++;
    if (Math.abs(d) > maxAbsDiff) maxAbsDiff = Math.abs(d);
  }
  log(
    `  raw bytes: ${differingBytes}/${bytesA.length} differ`
      + ` (max |Δ|=${maxAbsDiff} of 255). ${
        differingBytes === 0
          ? "WARNING: identical input bytes — generator/canvas problem"
          : "inputs are distinct ✓"
      }`,
  );

  // Step 1: verify the JS→Blob→fetch→SAB→wasm-memory copy path WITHOUT
  // running inference. getInput() returns &buffer0[65536]; layer 8 of the
  // model writes back into that region during invoke_inf (webapp memory
  // bug #2), so sampling the signature AFTER infer reads trampled
  // activations, not the input. The load_input envelope does the byte
  // copy and grabs the sig before invoke_inf is ever called.
  // Pass null as the revoke arg — we re-use these blob URLs for the
  // backbone-diff infers below, so the URLs must stay live until then.
  const urlA = canvasToBlobUrl(a.canvas);
  const sigA = await submitAndWait({ type: "load_input", url: urlA }, null);
  const urlB = canvasToBlobUrl(b.canvas);
  const sigB = await submitAndWait({ type: "load_input", url: urlB }, null);
  if (sigA.type !== "input_sig" || sigB.type !== "input_sig") {
    log("! load_input sig probe failed during verify");
    $("btn-verify").disabled = false;
    return;
  }

  // Compare what JS sent vs what wasm saw, for both samples. First 16
  // bytes is enough to detect any drift — they're the top-left corner of
  // the image and will differ between the person and scene canvases.
  const jsA16 = Array.from(bytesA.slice(0, 16));
  const jsB16 = Array.from(bytesB.slice(0, 16));
  let aMatch = true, bMatch = true;
  for (let i = 0; i < 16; i++) {
    if (jsA16[i] !== sigA.bytes[i]) aMatch = false;
    if (jsB16[i] !== sigB.bytes[i]) bMatch = false;
  }
  log(`  js bytes A[0..16]:  [${jsA16.join(",")}]`);
  log(`  wasm sig  A[0..16]: [${sigA.bytes.join(",")}] ${aMatch ? "✓" : "✗ MISMATCH"}`);
  log(`  js bytes B[0..16]:  [${jsB16.join(",")}]`);
  log(`  wasm sig  B[0..16]: [${sigB.bytes.join(",")}] ${bMatch ? "✓" : "✗ MISMATCH"}`);
  if (!aMatch || !bMatch) {
    log(`  ✗ INPUT PATH BUG: bytes JS handed off do not match what wasm`);
    log(`    saw at getInput(). The chain JS→Blob→fetch→arrayBuffer→SAB→`);
    log(`    wasmMemory copy is dropping or scrambling bytes.`);
    $("btn-verify").disabled = false;
    return;
  }

  // Step 2: input path is verified clean — now run two infers (no
  // training in between) on the same two samples and check whether
  // distinct inputs produce distinct logits. Same blob URLs so the
  // bytes wasm sees are identical to the ones we just verified.
  const replyA = await submitAndWait({ type: "infer", url: urlA }, urlA);
  const replyB = await submitAndWait({ type: "infer", url: urlB }, urlB);
  if (replyA.type !== "infer_result" || replyB.type !== "infer_result") {
    log("! infer failed during backbone-diff check");
    $("btn-verify").disabled = false;
    return;
  }
  const la = rawScoreVector(replyA);
  const lb = rawScoreVector(replyB);
  log(`  logits A (${CLASS_NAMES[PERSON]}): [${la.join(",")}]`);
  log(`  logits B (${CLASS_NAMES[SCENE]}):  [${lb.join(",")}]`);
  if (replyA.binary_scores && replyB.binary_scores) {
    log(`  2-class A: [${scoreVector(replyA).join(",")}]`);
    log(`  2-class B: [${scoreVector(replyB).join(",")}]`);
  }
  if (replyA.scores && replyB.scores) {
    log(`  raw int8 A: [${replyA.logits.join(",")}]`);
    log(`  raw int8 B: [${replyB.logits.join(",")}]`);
  }

  // Diff: how many of the NUM_CLASSES positions differ, and by how much.
  let nDiff = 0;
  let maxLogitDiff = 0;
  for (let i = 0; i < NUM_CLASSES; i++) {
    const d = la[i] - lb[i];
    if (d !== 0) nDiff++;
    if (Math.abs(d) > maxLogitDiff) maxLogitDiff = Math.abs(d);
  }
  log(`  score diff: ${nDiff}/${NUM_CLASSES} positions differ, max |Δ|=${fmtDelta(maxLogitDiff)}`);
  if (nDiff === 0) {
    log(`  ✗ BACKBONE BUG: identical logits despite ${differingBytes}-byte input diff.`);
    log(`    Either (a) wasm read cached bytes, or (b) the frozen backbone`);
    log(`    is producing identical layer-50 features for these inputs.`);
  } else if (maxLogitDiff < 3) {
    log(`  ⚠ Backbone barely responds: max score Δ < 3 across 10 classes.`);
    log(`    Features are technically distinct but the signal is far too`);
    log(`    weak for the trainable head to amplify into a class boundary.`);
  } else {
    log(`  ✓ Backbone IS input-dependent — different inputs ⇒ different`);
    log(`    layer-50 features ⇒ different scores. Training collapse is`);
    log(`    not an input/backbone problem; it's a training-dynamics problem.`);
  }

  await verifyClassSeparability();

  log(`── training-state verification ──`);
  const trainA = S.dataset.find((it) => !it.isTest && it.label === PERSON) || a;
  const trainB = S.dataset.find((it) => !it.isTest && it.label === SCENE) || b;
  const dbg0 = await readTrainDebug();
  logTrainDebug("after reset", dbg0);

  const baseA1 = await inferCanvas(a.canvas);
  const baseB1 = await inferCanvas(b.canvas);
  const baseA2 = await inferCanvas(a.canvas);
  if (baseA1.type === "infer_result" && baseB1.type === "infer_result" && baseA2.type === "infer_result") {
    log(
      `  baseline val A: pred=${CLASS_NAMES[binaryFromReply(baseA1)]} Δ=${fmtDelta(logitDelta(baseA1))} scores=[${
        scoreVector(baseA1).join(",")
      }] logits=[${baseA1.logits.join(",")}]`,
    );
    log(
      `  baseline val B: pred=${CLASS_NAMES[binaryFromReply(baseB1)]} Δ=${fmtDelta(logitDelta(baseB1))} scores=[${
        scoreVector(baseB1).join(",")
      }] logits=[${baseB1.logits.join(",")}]`,
    );
    log(
      `  repeat val A:   pred=${CLASS_NAMES[binaryFromReply(baseA2)]} Δ=${fmtDelta(logitDelta(baseA2))}`
        + ` same_logits=${JSON.stringify(baseA1.logits) === JSON.stringify(baseA2.logits) ? "yes" : "NO"}`,
    );
  }

  const beforeTrainA = await inferCanvas(trainA.canvas);
  log(
    `  train probe A before update: true=${CLASS_NAMES[trainA.label]}`
      + ` pred=${beforeTrainA.type === "infer_result" ? CLASS_NAMES[binaryFromReply(beforeTrainA)] : "ERR"}`
      + ` Δ=${beforeTrainA.type === "infer_result" ? fmtDelta(logitDelta(beforeTrainA)) : "?"}`,
  );
  const trainReplyA = await trainCanvas(trainA.canvas, trainA.label);
  log(`  train probe A update reply: ${trainReplyA.type}`);
  const dbgA = await readTrainDebug();
  logTrainDebug("after one person update", dbgA);
  const afterAonA = await inferCanvas(a.canvas);
  const afterAonB = await inferCanvas(b.canvas);
  if (afterAonA.type === "infer_result" && afterAonB.type === "infer_result") {
    log(
      `  after person update val A: pred=${CLASS_NAMES[binaryFromReply(afterAonA)]} Δ=${
        fmtDelta(logitDelta(afterAonA))
      } scores=[${scoreVector(afterAonA).join(",")}] logits=[${afterAonA.logits.join(",")}]`,
    );
    log(
      `  after person update val B: pred=${CLASS_NAMES[binaryFromReply(afterAonB)]} Δ=${
        fmtDelta(logitDelta(afterAonB))
      } scores=[${scoreVector(afterAonB).join(",")}] logits=[${afterAonB.logits.join(",")}]`,
    );
  }

  const beforeTrainB = await inferCanvas(trainB.canvas);
  log(
    `  train probe B before update: true=${CLASS_NAMES[trainB.label]}`
      + ` pred=${beforeTrainB.type === "infer_result" ? CLASS_NAMES[binaryFromReply(beforeTrainB)] : "ERR"}`
      + ` Δ=${beforeTrainB.type === "infer_result" ? fmtDelta(logitDelta(beforeTrainB)) : "?"}`,
  );
  const trainReplyB = await trainCanvas(trainB.canvas, trainB.label);
  log(`  train probe B update reply: ${trainReplyB.type}`);
  const dbgB = await readTrainDebug();
  logTrainDebug("after one scene update", dbgB);
  const afterBonA = await inferCanvas(a.canvas);
  const afterBonB = await inferCanvas(b.canvas);
  if (afterBonA.type === "infer_result" && afterBonB.type === "infer_result") {
    log(
      `  after scene update val A: pred=${CLASS_NAMES[binaryFromReply(afterBonA)]} Δ=${
        fmtDelta(logitDelta(afterBonA))
      } scores=[${scoreVector(afterBonA).join(",")}] logits=[${afterBonA.logits.join(",")}]`,
    );
    log(
      `  after scene update val B: pred=${CLASS_NAMES[binaryFromReply(afterBonB)]} Δ=${
        fmtDelta(logitDelta(afterBonB))
      } scores=[${scoreVector(afterBonB).join(",")}] logits=[${afterBonB.logits.join(",")}]`,
    );
  }

  // Default training updates the stable fp32 prototype head over frozen
  // generated features; Path C sparse updates are covered by the smoke test.
  if (dbgB && dbgB.binary_updates >= 2 && dbgB.binary_changed > 0) {
    log(
      `  ✓ prototype head trained: ${dbgB.binary_changed} feature means changed`
        + ` over ${dbgB.binary_updates} updates`,
    );
  } else if (dbgB) {
    log(`  ✗ prototype head did not move — check pooled-feature readout.`);
  }

  const r2 = await submitAndWait({ type: "reset" }, null);
  log(`  cleanup reset: ${r2.type}`);

  // Cleanup display state — the reset wiped predictions; show the gallery
  // in untouched state again.
  S.runningCorrect = 0;
  S.runningTotal = 0;
  S.lastTestAcc = null;
  for (const item of S.dataset) {
    item.lastPred = null;
    item.lastLogitDelta = null;
    item.lastFullArgmax = null;
  }
  renderGallery();
  $("btn-verify").disabled = false;
}

// Pull the int8 bytes a canvas produces without wrapping in a Blob — same
// math as canvasToInt8Blob but returns the array directly for byte-level
// comparison.
function canvasToInt8Bytes(canvas) {
  const ctx = canvas.getContext("2d");
  const pixels = ctx.getImageData(0, 0, IMG_W, IMG_H).data;
  const buf = new Int8Array(IMG_W * IMG_H * 3);
  let j = 0;
  for (let i = 0; i < IMG_W * IMG_H; i++) {
    buf[j++] = pixels[i * 4 + 0] - 128;
    buf[j++] = pixels[i * 4 + 1] - 128;
    buf[j++] = pixels[i * 4 + 2] - 128;
  }
  return buf;
}

$("btn-verify").addEventListener("click", verifyInput);

// Pretrained-backbone sanity check. Pushes a solid-black and a solid-white
// 128×128 RGB frame through the TinyEngine model and reports the per-class
// logit delta plus the pooled-feature delta. With the generated sparse-update
// graph in place this should show:
//   - Pooled features differ on many of the 16 sampled channels (the
//     backbone is responsive)
//   - 2-class logits differ by a non-trivial amount (the head encodes
//     real input dependence)
// A near-zero delta after this swap would point to a regression in the
// inference path (codegen substitution, input format, etc) rather than
// the old "broken pretrained .pkl" failure mode.
async function verifyExtremes() {
  if (!S.controller) return;
  $("btn-extremes").disabled = true;
  log(`── extremes (black vs white) verification ──`);

  // Reset so we're testing the frozen pretrained backbone, not whatever
  // state the head was in after the last training session.
  const r = await submitAndWait({ type: "reset" }, null);
  if (!r || r.type !== "reset_ack") {
    log("! reset failed; aborting extremes verify");
    $("btn-extremes").disabled = false;
    return;
  }

  const mkSolid = (rgb) => {
    const c = document.createElement("canvas");
    c.width = IMG_W;
    c.height = IMG_H;
    const ctx = c.getContext("2d");
    ctx.fillStyle = `rgb(${rgb},${rgb},${rgb})`;
    ctx.fillRect(0, 0, IMG_W, IMG_H);
    return c;
  };
  const blackCanvas = mkSolid(0); // → all bytes = -128 after pixel-128
  const whiteCanvas = mkSolid(255); // → all bytes = +127 after pixel-128

  // Capture the wasm-side input signature pristine (before invoke_inf
  // trampples buffer0[65536..]) so we can confirm what the backbone
  // actually sees vs what we sent.
  const urlBlack = canvasToBlobUrl(blackCanvas);
  const sigBlack = await submitAndWait({ type: "load_input", url: urlBlack }, null);
  const urlWhite = canvasToBlobUrl(whiteCanvas);
  const sigWhite = await submitAndWait({ type: "load_input", url: urlWhite }, null);
  if (sigBlack.type !== "input_sig" || sigWhite.type !== "input_sig") {
    log("! load_input sig probe failed during extremes verify");
    $("btn-extremes").disabled = false;
    return;
  }
  log(`  wasm sig black[0..8]: [${sigBlack.bytes.slice(0, 8).join(",")}] (expect all -128)`);
  log(`  wasm sig white[0..8]: [${sigWhite.bytes.slice(0, 8).join(",")}] (expect all +127)`);

  // Infer on each — re-using urlBlack / urlWhite is fine, payload is
  // deterministic. Right after each infer, grab the pooled feature sig
  // (first 16 of the 160 int8 values at buffer0[34592], the input to
  // layer 50's classifier head). Pooled sig must be read AFTER invoke
  // so it reflects layer 49's avg-pool output; layer 50 does not write
  // back into [34592..34752] so it's stable after invoke completes.
  const replyBlack = await submitAndWait({ type: "infer", url: urlBlack }, urlBlack);
  const poolBlack = await submitAndWait({ type: "get_pooled_sig" }, null);
  const replyWhite = await submitAndWait({ type: "infer", url: urlWhite }, urlWhite);
  const poolWhite = await submitAndWait({ type: "get_pooled_sig" }, null);
  if (
    replyBlack.type !== "infer_result" || replyWhite.type !== "infer_result"
    || poolBlack.type !== "pooled_sig" || poolWhite.type !== "pooled_sig"
  ) {
    log("! infer or pooled-sig probe failed during extremes verify");
    $("btn-extremes").disabled = false;
    return;
  }

  // Pooled-feature delta. These are layer 49's avg-pool output, fed
  // directly into the classifier head. If max |Δ| here is ~0 the
  // backbone has fully collapsed before pool; if it's healthy (5+) the
  // backbone is working and the score readout / head is suspect.
  let poolMaxDiff = 0;
  let poolNDiff = 0;
  for (let i = 0; i < 16; i++) {
    const d = poolBlack.bytes[i] - poolWhite.bytes[i];
    if (d !== 0) poolNDiff++;
    if (Math.abs(d) > poolMaxDiff) poolMaxDiff = Math.abs(d);
  }
  log(`  pooled black[0..16]: [${poolBlack.bytes.join(",")}]`);
  log(`  pooled white[0..16]: [${poolWhite.bytes.join(",")}]`);
  log(`  pooled diff: ${poolNDiff}/16 positions differ, max |Δ|=${poolMaxDiff} (int8 units)`);
  if (poolMaxDiff <= 1) {
    log(`  ⚠ Pooled features are essentially identical for black vs white —`);
    log(`    the backbone has collapsed BEFORE the avg-pool layer. The`);
    log(`    classifier head can't recover from this; no training of head`);
    log(`    weights will help. Issue is in the conv layers themselves,`);
    log(`    most likely an input-preprocessing / scale-and-zero-point`);
    log(`    mismatch in the early convolutions.`);
  } else if (poolMaxDiff < 16) {
    log(`  ⚠ Pooled features differ weakly (max |Δ|=${poolMaxDiff} of 255).`);
    log(`    Backbone is partially responsive but the signal is small.`);
  } else {
    log(`  ✓ Pooled features differ strongly (max |Δ|=${poolMaxDiff}) — the`);
    log(`    backbone IS encoding input-dependent information. The score`);
    log(`    readout / final42_unclipped_score is the suspect (it has a`);
    log(`    known +1 vs +34 zero-point bug — should fix it).`);
  }

  const sBlack = scoreVector(replyBlack);
  const sWhite = scoreVector(replyWhite);
  log(`  scores black: [${sBlack.map((v) => v.toFixed(2)).join(",")}]`);
  log(`  scores white: [${sWhite.map((v) => v.toFixed(2)).join(",")}]`);

  let nDiff = 0;
  let maxAbsDiff = 0;
  for (let i = 0; i < sBlack.length; i++) {
    const d = sBlack[i] - sWhite[i];
    if (Math.abs(d) > 0.5) nDiff++;
    if (Math.abs(d) > maxAbsDiff) maxAbsDiff = Math.abs(d);
  }
  log(`  score diff: ${nDiff}/${NUM_CLASSES} positions differ by >0.5, max |Δ|=${maxAbsDiff.toFixed(2)}`);

  // Combined verdict using BOTH the pooled-feature delta (backbone
  // health) and the head's score delta (head's alignment with this
  // particular input direction). With a VWW-pretrained head, the
  // weights are tuned for natural-photo person/no-person variation,
  // not adversarial solid colors — so it's normal to see significant
  // pooled-feature variation but tiny score variation on extremes.
  if (poolMaxDiff <= 1) {
    log(`  ✗ BACKBONE BROKEN: pooled features barely differ between`);
    log(`    extreme inputs (max |Δ|=${poolMaxDiff}). The conv chain is not`);
    log(`    encoding input information. Candidates: BGR/RGB channel order,`);
    log(`    wrong spatial resolution, mean/std normalisation mismatch, or`);
    log(`    a stale codegen.`);
  } else if (maxAbsDiff < 3 && poolMaxDiff >= 2) {
    log(`  ✓ BACKBONE OK, head insensitive to extremes`);
    log(`    Pooled features vary on ${poolNDiff}/16 channels — the conv`);
    log(`    chain encodes input differences. The head's 320 weights happen`);
    log(`    to project these particular synthetic differences onto a near-`);
    log(`    zero logit delta, which is expected: the pretrained VWW head`);
    log(`    was tuned for natural-photo person/no-person variation, not`);
    log(`    adversarial solid colours. Test with real photos — the head`);
    log(`    should produce meaningful logit differences there.`);
  } else if (maxAbsDiff < 20) {
    log(`  ⚠ Backbone responsive, head moderately differentiates`);
    log(`    extremes (max |Δ|=${maxAbsDiff.toFixed(2)}). Some signal makes it`);
    log(`    through to the head; real photos should produce stronger`);
    log(`    logit deltas.`);
  } else {
    log(`  ✓ Backbone + head BOTH strongly differentiate extremes`);
    log(`    (pooled max |Δ|=${poolMaxDiff}, score max |Δ|=${maxAbsDiff.toFixed(2)}).`);
    log(`    The model is fully responsive — real photos should classify`);
    log(`    correctly out of the box. The prototype head can refine`);
    log(`    the person/scene boundary from uploaded examples.`);
  }

  // Reset display state — verifier rest wiped predictions.
  S.runningCorrect = 0;
  S.runningTotal = 0;
  S.lastTestAcc = null;
  for (const item of S.dataset) {
    item.lastPred = null;
    item.lastLogitDelta = null;
    item.lastFullArgmax = null;
  }
  renderGallery();
  $("btn-extremes").disabled = false;
}

$("btn-extremes").addEventListener("click", verifyExtremes);

// One-shot held-out evaluation. Runs the same evaluateTest() loop the
// training flow does at end-of-epoch, but standalone — no reset, no
// training step. Useful for checking the current restored generated-head plus
// prototype readout before deciding whether to train more.
async function evaluateTestStandalone() {
  if (!S.controller) return;
  const testCount = S.dataset.filter((it) => it.isTest).length;
  if (testCount === 0) {
    log(`! no held-out test samples loaded — upload some via the`);
    log(`  'validation/test' uploads or regenerate the synthetic dataset`);
    return;
  }
  $("btn-eval").disabled = true;
  $("btn-train").disabled = true;
  log(`── held-out evaluation (no training) ──`);
  S.stop = false;
  const result = await evaluateTest();
  if (!result) {
    log(`! evaluation failed`);
  } else {
    const pct = (n) => (n * 100).toFixed(1);
    log(
      `  test_acc=${pct(result.accOverall)}% `
        + `(${result.correctPerson + result.correctScene}/`
        + `${result.totalPerson + result.totalScene}) `
        + `[${CLASS_NAMES[PERSON]}=${result.correctPerson}/${result.totalPerson}, `
        + `${CLASS_NAMES[SCENE]}=${result.correctScene}/${result.totalScene}] `
        + `preds: ${CLASS_NAMES[PERSON]}=${result.predCount0}, `
        + `${CLASS_NAMES[SCENE]}=${result.predCount1}`,
    );
    S.lastTestAcc = result.accOverall;
    // Refresh the m-acc readout if the trainer UI has been touched.
    const valPct = S.lastTestAcc != null
      ? (S.lastTestAcc * 100).toFixed(1)
      : null;
    setMetric("m-acc", `t — / v ${valPct ?? "—"}%`);
  }
  $("btn-eval").disabled = false;
  $("btn-train").disabled = false;
}
$("btn-eval").addEventListener("click", evaluateTestStandalone);
// Live LR/BLR change: when the user edits either input, push the new pair
// to the wasm so an already-running training (or a paused one) picks up
// the new rate on its next step. Debounce-light by just sending on
// "change" (= focus out / Enter) rather than every keystroke.
$("cfg-lr").addEventListener("change", pushLrFromUi);
$("cfg-blr").addEventListener("change", pushLrFromUi);
$("cfg-binary-lr").addEventListener("change", pushLrFromUi);
$("btn-train").addEventListener("click", runTraining);
$("btn-stop").addEventListener("click", () => {
  S.stop = true;
});

// Reset the wasm model back to its initial post-init weights. Also clears
// the demo-side running training-accuracy counters and per-tile predictions
// so the next training run starts with a clean slate — same effect as a
// page reload, just without re-instantiating the worker.
$("btn-reset-model").addEventListener("click", async () => {
  if (!S.controller) return;
  $("btn-reset-model").disabled = true;
  log("↺ resetting model weights to initial snapshot…");
  const reply = await submitAndWait({ type: "reset" }, null);
  if (reply && reply.type === "reset_ack") {
    S.runningCorrect = 0;
    S.runningTotal = 0;
    S.lastTestAcc = null;
    S.trainLosses = [];
    S.epochAccs = [];
    setMetric("m-epoch", "—");
    setMetric("m-step", "—");
    setMetric("m-acc", "—");
    setMetric("m-loss", "—");
    for (const item of S.dataset) {
      item.lastPred = null;
      item.lastLogitDelta = null;
      item.lastFullArgmax = null;
    }
    renderGallery();
    drawLossCurve();
    log("✓ model reset");
  } else {
    log("! reset did not acknowledge");
  }
  $("btn-reset-model").disabled = false;
});

// Initial dataset before wasm is ready.
regenerate();

// Start the wasm module headlessly. The controller comes back via onReady
// once the wasm has issued its `ready` envelope.
run({
  ui: false,
  waitUntilExit: true,
  onReady: (controller) => {
    S.controller = controller;
    $("btn-train").disabled = false;
    $("btn-reset-model").disabled = false;
    $("btn-verify").disabled = false;
    $("btn-extremes").disabled = false;
    $("btn-eval").disabled = false;
    setAgentStatus("wasm ready · click Start training", true);
    log("✓ wasm worker ready");
  },
  onReply: handleReply,
}).then(
  () => log("· wasm worker exited"),
  (e) => log(`! wasm worker error: ${e?.message ?? e}`),
);

setAgentStatus("instantiating wasm worker…");
