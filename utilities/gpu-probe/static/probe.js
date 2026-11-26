// The probe page's script: it runs every probe set the browser offers, shows each GPU's row and signatures, and
// submits them to the server that serves the page, or proposes them on GitHub where none does.
// skipcq: JS-0833 -- committed ES module; the analyzer's script-mode parse is a false positive
import { loadProbes } from "/load_probes.js";
import { judged, proposals } from "/propose.js";
import { register } from "/register.js";

window.gpuProbeStarted = true;

// The probe sets, once loaded: `probe_canvas`, `probe_webgl1`, `probe_webgl2` and `probe_webgpu`.
let probes = null;

const field = (id) => document.getElementById(id);
const status = field("status");

// Whether a gpu-probe-server serves this page, which it is told by the registry being there to read. A static host,
// such as a Vercel deploy that Tor Browser can reach where it cannot reach a LAN, has no registry to add to, so there
// the submissions are proposed on GitHub instead. It is settled as the run starts.
let served = false;

// Tell a driver how a run went: in the console, one line per line of the status, each opening `gpu-probe: `, and to
// the server, which logs them. A browser with no WebDriver is read from whichever of the two reaches it. A post the
// server never takes, or does not answer within a few seconds, leaves the console lines standing, and is no failure of
// the probes themselves. It answers whether the server took them. The wait is an AbortController's rather than
// `AbortSignal.timeout`, which Chrome lacks before 103: there the post threw, so an old phone's failure never reached
// the server.
const statusWait = 5000;
const tell = async (text) => {
  const lines = text.split("\n");
  for (const line of lines) console.log(`gpu-probe: ${line}`);
  if (!served) return false;
  const waited = new AbortController();
  const timer = setTimeout(() => waited.abort(), statusWait);
  try {
    const response = await fetch("/status", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ status: lines }),
      signal: waited.signal,
    });
    return response.ok;
  } catch (error) {
    console.log(`gpu-probe: the server was not told: ${String(error)}`);
    return false;
  } finally {
    clearTimeout(timer);
  }
};

// The high-entropy hints named, which only Chromium gives; elsewhere nothing.
const highEntropy = (names) => navigator.userAgentData?.getHighEntropyValues?.(names);

// Each brand a Chromium browser names in its hints, by its full version.
const brandVersions = async () => {
  const hints = await highEntropy(["fullVersionList"]);
  return new Map((hints?.fullVersionList ?? []).map((brand) => [brand.brand, brand.version]));
};

// The registry's name and version for a Chromium browser, from its brands, or null for a browser that names none.
// One that embeds Chromium, such as an Electron app, names only Chromium there, and its own product just before Chrome
// in the user agent, which the version then carries.
const chromiumOf = (agent, versions, chrome) => {
  const branded = [
    ["Google Chrome", "chrome"],
    ["Microsoft Edge", "edge"],
    ["Opera", "opera"],
  ];
  for (const [brand, name] of branded) {
    if (versions.has(brand)) return [name, versions.get(brand)];
  }
  if (!versions.has("Chromium")) return null;
  const host = agent.match(/\) ([A-Za-z]+)\/([\d.]+) Chrome\//);
  return ["chromium", host ? `${host[1]} ${host[2]} (Chromium ${chrome})` : chrome];
};

// The registry's name and version for a browser, from the token its user agent names it by.
const namedOf = (agent) => {
  const named = [
    ["samsung-internet", /SamsungBrowser\/([\d.]+)/],
    // Edge names itself `Edg` on the desktop, `EdgA` on Android and `EdgiOS` on iOS, beside a Chrome or Safari token,
    // so it is matched before those.
    ["edge", /Edg(?:A|iOS)?\/([\d.]+)/],
    ["firefox", /Firefox\/([\d.]+)/],
    ["chrome", /Chrome\/([\d.]+)/],
    // On iOS every browser is WebKit, and Chrome and Firefox name themselves by these tokens instead.
    ["chrome", /CriOS\/([\d.]+)/],
    ["firefox", /FxiOS\/([\d.]+)/],
    ["safari", /Version\/([\d.]+).*Safari/],
  ];
  for (const [name, pattern] of named) {
    const match = agent.match(pattern);
    if (match) return [name, match[1]];
  }
  return ["unknown", ""];
};

// The registry's name for the browser, and its full version, from what it says it is.
// A Chromium browser names itself in its brands, and gives its full version only through the high-entropy hints.
const browserOf = async (agent) => {
  const versions = await brandVersions();
  const chrome = versions.get("Chromium") ?? agent.match(/Chrome\/([\d.]+)/)?.[1] ?? "";
  if (navigator.brave) return ["brave", chrome];
  return chromiumOf(agent, versions, chrome) ?? namedOf(agent);
};

// `text` split at its commas outside parentheses.
const topLevel = (text) => {
  const parts = [""];
  let depth = 0;
  for (const character of text) {
    if (character === "," && depth === 0) {
      parts.push("");
      continue;
    }
    if (character === "(") depth += 1;
    if (character === ")") depth -= 1;
    parts[parts.length - 1] += character;
  }
  return parts.map((part) => part.trim());
};

// The API a browser drives its GPU through, as the registry names it, from where ANGLE names it.
const apis = [
  [/Vulkan/, "vulkan"],
  [/OpenGL ES/, "opengl-es"],
  [/OpenGL/, "opengl"],
  [/Direct3D11|D3D11/, "d3d11"],
  [/Metal/, "metal"],
];

// The registry's GPU vendor for each name a driver gives its vendor by, as ANGLE passes it on or WebGL reports it,
// lowercased; a name the registry already uses maps to itself. A Map rather than an object, so that looking a name up
// needs nothing newer than the oldest browsers the page runs on, and no inherited key is ever mistaken for one.
const vendorNames = new Map([
  ["arm", "arm"],
  ["ati technologies inc.", "amd"],
  ["imagination technologies", "imgtec"],
  ["nvidia", "nvidia"],
  ["nvidia corporation", "nvidia"],
  ["qualcomm", "qualcomm"],
]);

// The GPU a renderer ANGLE does not wrap names, which is the driver's own. On Android that driver is OpenGL ES, which
// Chrome before ANGLE reached it, such as Chrome 95 on Android 5, drew through alone, and WebGL gives the driver's
// vendor beside it, by the driver's name for it: only those the registry names a GPU vendor by are taken.
const driverGpu = (unmasked, similar) => {
  const { context, debug, renderer } = unmasked;
  const android = renderer && field("os").value === "android";
  const named = debug && android ? String(context.getParameter(debug.UNMASKED_VENDOR_WEBGL)) : "";
  const driverVendor = named.toLowerCase();
  const vendor = vendorNames.has(driverVendor) ? driverVendor : "";
  const backend = android ? "opengl-es" : "unknown";
  return similar ? { vendor, model: "", backend, reported: renderer } : { vendor, model: renderer, backend };
};

// The GPU ANGLE names inside its renderer string, `inner` being what its parentheses hold.
const angleGpu = (inner, similar, renderer) => {
  const [vendor, named = "", ...rest] = topLevel(inner);
  const api = apis.find(([pattern]) => [named, ...rest].some((part) => pattern.test(part)));
  const vulkan = named.match(/^Vulkan [\d.]+ \((.*?)(?: \(0x[0-9A-Fa-f]+\))?\)$/);
  const direct3d = named.match(/^(.*?)(?: \(0x[0-9A-Fa-f]+\))? Direct3D\w* vs_\w+ ps_\w+$/);
  const listed = vulkan ?? direct3d;
  const model = listed ? listed[1].replace(new RegExp(`^${vendor} `, "i"), "") : named;
  const backend = api ? api[1] : "unknown";
  if (similar) return { vendor, model: "", backend, reported: renderer };
  return { vendor, model, backend };
};

// The GPU WebGL reports, where the browser unmasks it: its vendor, its name, and the API the browser drives it
// through. ANGLE gives them as "ANGLE (vendor, model, API)" over OpenGL, and as
// "ANGLE (vendor, Vulkan 1.4 (vendor model (0xid)), driver)" over Vulkan, and as
// "ANGLE (vendor, vendor model (0xid) Direct3D11 vs_5_0 ps_5_0, D3D11)" over Direct3D on Windows.
// Firefox names a stand-in GPU of the same vendor and API instead, followed by ", or similar": its vendor and API are
// still the GPU's, but its model is not, so the string is kept only as what was reported.
const webglGpu = (kind) => {
  const context = document.createElement("canvas").getContext(kind);
  const debug = context?.getExtension("WEBGL_debug_renderer_info");
  const renderer = debug ? context.getParameter(debug.UNMASKED_RENDERER_WEBGL) : "";
  const similar = renderer.match(/^(.*), or similar$/);
  const angle = (similar ? similar[1] : renderer).match(/^ANGLE \((.*)\)$/);
  if (!angle) return driverGpu({ context, debug, renderer }, similar);
  return angleGpu(angle[1], similar, renderer);
};

// An iPhone's agent says "like Mac OS X", and an iPad's since iPadOS 13 is a Mac's outright; only a touch screen tells
// that iPad from a Mac, which has none.
const isIos = (agent) => /iPhone|iPad|iPod/.test(agent) || (/Macintosh/.test(agent) && navigator.maxTouchPoints > 1);

// Each system the registry names, by how a user agent shows it, in the order they are tried: an iPad's agent can say
// Macintosh, and Android's says Linux.
const systems = [
  ["android", (agent) => /Android/.test(agent)],
  ["windows", (agent) => /Windows/.test(agent)],
  ["ios", isIos],
  ["macos", (agent) => /Mac OS X/.test(agent)],
  ["linux", (agent) => /Linux/.test(agent)],
];

const systemOf = (agent) => systems.find(([, shows]) => shows(agent))?.[0] ?? "unknown";

// The API a browser's WebGPU runs on, which it does not say: the one each browser uses on that system, where there is
// only one. On Windows Chrome and Edge's Dawn and Firefox's wgpu each run on Direct3D 12.
const webgpuBackendOf = (os) =>
  ({
    linux: "vulkan",
    android: "vulkan",
    macos: "metal",
    ios: "metal",
    windows: "d3d12",
  })[os] ?? "unknown";

// Only Chromium says which CPU it runs on; everywhere else the field is left for the person submitting.
const architectureOf = async () => {
  const hints = await highEntropy(["architecture", "bitness"]);
  if (hints?.bitness !== "64") return "unknown";
  return { x86: "x86_64", arm: "aarch64" }[hints.architecture] ?? "unknown";
};

// The name of the Windows release whose platform version is `version`: Windows 11 counts from 13 and Windows 10 from 1.
const windowsRelease = (version) => {
  const major = Number.parseInt(version, 10);
  if (major >= 13) return "11";
  return major >= 1 ? "10" : "";
};

// The device's model and the system's version, which only Chromium says, and on Android alone the model: a code such
// as SM-G973F, which a person can name the phone from. Windows gives its version as a number, so it is given as the
// name of the release instead.
const systemHintsOf = async () => {
  const hints = (await highEntropy(["model", "platformVersion"])) ?? {};
  const model = hints.model ?? "";
  const version = hints.platformVersion ?? "";
  const windows = navigator.userAgentData?.platform?.startsWith("Windows");
  return { model, version: windows ? windowsRelease(version) : version };
};

// The device the row names, as a label puts it after the system, or nothing when the row names none.
const onDevice = () => {
  const device = field("device").value.trim();
  return device ? `, ${device}` : "";
};

// The registry takes a vendor it has no name for as the name the browser gives, so only a vendor the browser does not
// name at all is unknown. A driver's long name for a vendor the registry names is taken as that name.
const vendorOf = (name) => {
  const given = name.trim().toLowerCase();
  return vendorNames.get(given) ?? (given || "unknown");
};

// What the page was opened with: `device` names the machine and `os_version` the system's version, in place of what
// the browser says of either, `defence` the privacy setting the browser has in force, `environment` the driver settings
// the browser was started under, such as `INTEL_DEBUG=nofc`, which a page cannot see, and `submit` submits every probe
// set once probed. `browser`, `browser_version` and `window` say what a driver knows and the page cannot: LibreWolf,
// Mullvad Browser and Tor Browser each report themselves as Firefox, and Mullvad Browser always browses privately.
// `brand_version` is the browser's own version where it is not the engine's: Brave reports only its Chromium version,
// which the row records, so its own goes in the labels alone.
const query = new URLSearchParams(location.search);
const brandVersion = query.get("brand_version") ?? "";

const fillShared = async () => {
  const system = await systemHintsOf();
  field("device").value = query.get("device") ?? system.model;
  field("os_version").value = query.get("os_version") ?? system.version;
  field("defence").value = query.get("defence") ?? "";
  field("environment").value = query.get("environment") ?? "";
  const [browser, version] = await browserOf(navigator.userAgent);
  field("browser").value = query.get("browser") ?? browser;
  field("browser_version").value = query.get("browser_version") ?? version;
  field("window").value = query.get("window") ?? field("window").value;
  field("os").value = systemOf(navigator.userAgent);
  field("cpu_arch").value = await architectureOf();
};

// The shared part of the row as the form holds it, leaving out free text left empty.
const sharedRow = () => {
  const row = {};
  for (const id of ["browser", "browser_version", "os", "os_version", "cpu_arch"]) {
    if (field(id).value.trim()) row[id] = field(id).value.trim();
  }
  return row;
};

// A row for one GPU: the shared part, with that GPU's fields where the registry puts them.
const rowFor = (gpuFields) => {
  const row = sharedRow();
  for (const name of ["gpu", "gpu_version", "backend", "reported"]) {
    const value = gpuFields[name].value.trim();
    if (value) row[name] = value;
  }
  row.window = field("window").value;
  for (const id of ["device", "defence", "environment"]) {
    if (field(id).value.trim()) row[id] = field(id).value.trim();
  }
  return row;
};

// A signature's submission as JSON text: its results, or for one the browser answered with noise, withheld with the
// reason.
const submission = (signature, gpuFields) => {
  const body = {
    interface: signature.interface,
    label: gpuFields.label.value,
    seen: rowFor(gpuFields),
  };
  if (signature.reason) {
    body.blocked = true;
    body.reason = signature.reason;
  } else {
    // An object keeps its keys in the order they are added, so the results go in in output order.
    body.results = {};
    for (const [name, word] of signature.results) {
      body.results[name] = word;
    }
  }
  return JSON.stringify(body);
};

const submit = async (signature, gpuFields, outcome) => {
  const { kind, message } = await register(submission(signature, gpuFields));
  outcome.className = kind;
  outcome.textContent = message;
  return kind;
};

// A signature's section, with its submit button where a server serves the page. It gives back its submission's line
// as it stands, and the submitter, which says how the submission went: the registry's kind of answer, and a line
// naming the GPU and the interface.
const showSignature = (signature, gpuFields, into, title) => {
  const section = document.createElement("section");
  const heading = document.createElement("h3");
  heading.textContent = signature.interface;
  const traits = document.createElement("p");
  traits.textContent = signature.reason ?? (signature.traits || signature.fingerprint);
  const lines = document.createElement("pre");
  lines.textContent = signature.describe.join("\n");
  const outcome = document.createElement("p");
  section.append(heading, traits);
  if (served) {
    const button = document.createElement("button");
    button.textContent = `Submit ${signature.interface}`;
    button.addEventListener("click", () => submit(signature, gpuFields, outcome));
    section.append(button, outcome);
  }
  section.append(lines);
  into.append(section);
  return {
    line: () => submission(signature, gpuFields),
    submit: async () => {
      const kind = await submit(signature, gpuFields, outcome);
      return {
        kind,
        line: `${title}, ${signature.interface}: ${outcome.textContent}`,
      };
    },
  };
};

// One GPU's section: its part of the row, editable, then each signature it gave with its own submit button.
const showGpu = (title, values, answer) => {
  const section = document.createElement("section");
  const heading = document.createElement("h2");
  heading.textContent = title;
  const fieldset = document.createElement("fieldset");
  const legend = document.createElement("legend");
  legend.textContent = "This GPU's part of the row";
  fieldset.append(legend);
  const gpuFields = {};
  for (const name of ["label", "gpu", "gpu_version", "backend", "reported"]) {
    const label = document.createElement("label");
    label.textContent = name;
    const input = document.createElement("input");
    input.value = values[name] ?? "";
    input.size = name === "label" ? 80 : 40;
    label.htmlFor = input.id = `${title}-${name}`;
    gpuFields[name] = input;
    fieldset.append(label, input);
  }
  section.append(heading, fieldset);
  if (answer.problems.length) {
    const problems = document.createElement("p");
    problems.className = "bad";
    problems.textContent = `Not probed: ${answer.problems.join("; ")}`;
    section.append(problems);
  }
  const shown = [...answer.signatures, ...(answer.withheld ?? [])];
  const submitters = shown.map((signature) => showSignature(signature, gpuFields, section, title));
  if (answer.report?.length) {
    const report = document.createElement("pre");
    report.textContent = answer.report.join("\n");
    section.append(report);
  }
  field("gpus").append(section);
  return submitters;
};

// The device a downloaded file is named for: the one the row names, or else the kind of Apple device the browser says
// it is, or else its system, lowercased with each run of other characters than letters and digits a hyphen.
const guessedDevice = () => {
  const kind = navigator.userAgent.match(/iPhone|iPad|iPod/)?.[0];
  const named = field("device").value.trim() || kind || field("os").value;
  return named
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-|-$/g, "");
};

// A proposal as an item of the list, one of `parts`: its link, which opens a pull request of its own, then the file
// it makes, to read before following it. A file too long for GitHub to take from a link is given to add by hand
// instead.
const showProposal = (parts) => (proposal, index) => {
  const item = document.createElement("li");
  const part = parts > 1 ? `, part ${index + 1} of ${parts}` : "";
  const numbered = parts > 1 ? `-${index + 1}` : "";
  const downloaded = `gpu-probe-${guessedDevice() || "unknown"}${numbered}.txt`;
  const sets = `${proposal.count} probe set${proposal.count > 1 ? "s" : ""}`;
  if (proposal.opens) {
    const link = document.createElement("a");
    link.href = proposal.link;
    link.target = "_blank";
    link.rel = "noopener";
    link.textContent = `Propose ${sets} on GitHub${part}`;
    item.append(link);
  } else {
    const tooLong = document.createElement("p");
    const byHand = "add it to the repository by hand.";
    tooLong.className = "bad";
    tooLong.textContent = `${sets}${part} make a file too long for GitHub to take from a link: ${byHand}`;
    item.append(tooLong);
  }
  // The file to download, for emailing in place of a pull request. A data URL rather than a blob one, as the list is
  // made again on every edit and a blob URL would hold its file until the page closed.
  const download = document.createElement("a");
  const encoded = encodeURIComponent(proposal.text);
  download.href = `data:text/plain;charset=utf-8,${encoded}`;
  download.download = downloaded;
  download.textContent = `Download ${downloaded} to email`;
  item.append(" ", download);
  const file = document.createElement("details");
  const summary = document.createElement("summary");
  summary.textContent = `The file: ${proposal.name}`;
  const text = document.createElement("pre");
  text.textContent = proposal.text;
  file.append(summary, text);
  item.append(file);
  return item;
};

// The name a GPU's label gives it: its model, or the vendor a stand-in was named for, or failing both, an unknown GPU.
const gpuName = (masked, model, stood) => (masked ? "unknown GPU" : model || stood || "unknown GPU");

// A GPU's part of the row, and the label an entry first seen on it would take.
// `reported` is what the browser named in place of the GPU, where it named a stand-in.
const gpuValues = (vendor, model, backend, api, reported = "") => {
  const os = field("os").value;
  // The version is the row's to record; the label names only the browser, which keeps it to one line, and the
  // browser's own version where that is not the one the row records.
  const browser = [field("browser").value, brandVersion].filter(Boolean).join(" ");
  // WebKit reports every GPU as an Apple one, so off Apple's systems that is what it said, not what the GPU is.
  const masked = model === "Apple GPU" && !["macos", "ios"].includes(os);
  const stood = reported && vendor ? `${vendor} GPU` : "";
  const name = gpuName(masked, model, stood);
  return {
    label: `${name} through ${backend} on ${os}${onDevice()} -- ${api} on ${browser}`,
    gpu: masked ? "unknown" : vendorOf(vendor),
    gpu_version: masked ? "" : model,
    backend,
    reported: masked ? model : reported,
  };
};

// Each adapter a page can ask WebGPU for: the GPU it prefers for speed, the one it prefers for power, and its software
// fallback. A browser gives the same adapter to more than one of these where it has fewer GPUs.
const adapterChoices = [
  { power: "high-performance", fallback: false },
  { power: "low-power", fallback: false },
  { power: "", fallback: true },
];

// Run `measure`, giving back what it answered beside the adapter its WebGPU requests were given. wgpu asks for its
// adapter through the page's own `navigator.gpu`, so the request is watched there for the length of the run: the row
// then describes the very adapter the signatures came from, even if the browser would pick another one now. The
// request is amended to ask for the fallback adapter when `fallback` is set, since wgpu 29's WebGPU backend forwards a
// request's power preference to the browser but not `force_fallback_adapter`.
const measured = async (fallback, measure) => {
  const gpu = navigator.gpu;
  const request = gpu.requestAdapter.bind(gpu);
  let adapter = null;
  gpu.requestAdapter = async (options) => {
    const asked = fallback ? { ...options, forceFallbackAdapter: true } : options;
    adapter = await request(asked);
    return adapter;
  };
  try {
    return { answer: JSON.parse(await measure()), adapter };
  } finally {
    delete gpu.requestAdapter;
  }
};

// An adapter's model, as WebGPU describes it: SwiftShader by that name, and any other by its description or, failing
// that, its architecture.
const adapterModel = (info) => {
  const swiftShader = info.isFallbackAdapter && info.architecture === "swiftshader";
  return swiftShader ? "SwiftShader (software renderer)" : info.description || info.architecture;
};

// What an adapter's row and answers come to, as one string to compare another adapter's by.
const answersKey = (values, answer) =>
  JSON.stringify([
    values,
    answer.signatures.map(({ interface: name, results }) => [name, results]),
    (answer.withheld ?? []).map(({ interface: name, reason }) => [name, reason]),
  ]);

const probeWebGpu = async (submitters, problems) => {
  if (!navigator.gpu) {
    problems.push("the browser offers no WebGPU");
    return;
  }
  const seen = new Set();
  const answered = new Set();
  for (const { power, fallback } of adapterChoices) {
    const choice = fallback ? "fallback" : power;
    const { answer, adapter } = await measured(fallback, () => probes.probe_webgpu(power, fallback));
    if (!adapter) {
      problems.push(`WebGPU gave no ${choice} adapter`);
      continue;
    }
    const info = adapter.info;
    // Two choices given the same adapter are measured once. An adapter is told apart by every identifier WebGPU gives,
    // its model's `device` among them; one that gives none could be any GPU, so it is never skipped.
    const named = [info.vendor, info.architecture, info.description, info.device];
    const key = [...named, info.isFallbackAdapter].join("/");
    if (named.some(Boolean) && seen.has(key)) continue;
    seen.add(key);
    const model = adapterModel(info);
    const values = gpuValues(info.vendor, model, webgpuBackendOf(field("os").value), "WebGPU");
    // An adapter that gives no identifier is measured however many choices give it, and one that answers every probe
    // set as an earlier one did, with the same row, would only submit that adapter's results again.
    const sameAnswers = answersKey(values, answer);
    if (answered.has(sameAnswers)) continue;
    answered.add(sameAnswers);
    submitters.push(...showGpu(`WebGPU ${choice}: ${model}`, values, answer));
  }
};

// A label names the browser and system as their makers spell them, where a row gives its fixed name.
const spelled = new Map([
  ["gnome-web", "GNOME Web"],
  ["mullvad-browser", "Mullvad Browser"],
  ["samsung-internet", "Samsung Internet"],
  ["tor-browser", "Tor Browser"],
  ["ios", "iOS"],
  ["librewolf", "LibreWolf"],
  ["macos", "macOS"],
]);

// The field `id` names, as its maker spells it.
const spelledField = (id) => {
  const value = field(id).value;
  return spelled.get(value) ?? value.charAt(0).toUpperCase() + value.slice(1);
};

// Run every probe set, showing each GPU's section, and give back the submitters and why any set was not probed.
const probeAll = async () => {
  const submitters = [];
  const problems = [];
  // WebGL 1 is its own row: a browser can offer it while withholding WebGL 2, and name another GPU through it.
  const webgls = [
    ["webgl2", "WebGL 2", probes.probe_webgl2],
    ["webgl", "WebGL 1", probes.probe_webgl1],
  ];
  for (const [kind, title, probe] of webgls) {
    const webgl = webglGpu(kind);
    const webglValues = gpuValues(webgl.vendor, webgl.model, webgl.backend, title, webgl.reported);
    const webglAnswer = JSON.parse(probe());
    problems.push(...webglAnswer.problems);
    submitters.push(...showGpu(`${title}: ${webgl.model}`, webglValues, webglAnswer));
  }
  await probeWebGpu(submitters, problems);
  // The canvas fingerprints say nothing of which GPU drew them, so their row names none.
  const canvasAnswer = JSON.parse(probes.probe_canvas());
  problems.push(...canvasAnswer.problems);
  const version = brandVersion || field("browser_version").value;
  const canvasValues = {
    label: `${spelledField("browser")} ${version} on ${spelledField("os")}${onDevice()} -- canvas 2D`,
    gpu: "unknown",
    gpu_version: "",
    backend: "unknown",
  };
  submitters.push(...showGpu("Canvas 2D", canvasValues, canvasAnswer));
  // Whether a canvas fingerprint is noise is read from these lines, so a driver is told them too.
  for (const shown of [...canvasAnswer.signatures, ...canvasAnswer.withheld]) {
    await tell(shown.describe.map((line) => `${shown.interface}: ${line}`).join("\n"));
  }
  await tell(canvasAnswer.report.map((line) => `read path ${line}`).join("\n"));
  return { submitters, problems };
};

// What the registry makes of the judgements, as one line: how many it already records, how many are new and how many
// are seen somewhere new, then which those are.
const judgedText = (judgements, proposed) => {
  const count = (kind) => judgements.filter((judgement) => judgement.kind === kind).length;
  const recorded = `the registry already records ${count("recorded")}`;
  const kinds = `${count("new")} are new signatures, and ${count("sighting")} are known signatures seen somewhere new`;
  const sightings = proposed
    .filter(({ kind }) => kind === "sighting")
    .map(({ submission: given, label }) => `${given.interface} (${label})`);
  const named = sightings.length ? ` Seen somewhere new: ${sightings.join("; ")}.` : "";
  return `Of ${judgements.length} probe sets, ${recorded}, ${kinds}.${named}`;
};

// Propose what the run found on GitHub, as a page no server serves does. The registry the site was built with, which
// every probe set is held against so that only what it does not record is proposed, is read first; a site without it
// proposes everything, and says so. The proposals follow every edit to a row, so what is proposed is what the rows
// say. Making them is asynchronous, so only the latest edit's are shown.
const propose = async (submitters, missing) => {
  const known = await fetch("/signatures.json").then(
    (response) => (response.ok ? response.json() : null),
    () => null,
  );
  field("submit-all").hidden = true;
  field("propose").hidden = false;
  let edits = 0;
  const fill = async () => {
    edits += 1;
    const edit = edits;
    const submissions = submitters.map((shown) => JSON.parse(shown.line()));
    const judgements = known
      ? judged(submissions, known)
      : submissions.map((given) => ({ submission: given, kind: "new" }));
    const proposed = judgements.filter(({ kind }) => kind !== "recorded");
    const made = await proposals(proposed.map(({ submission: given }) => given));
    if (edit !== edits) return;
    const unread = "The registry could not be read, so every probe set is proposed.";
    field("judged").textContent = known ? judgedText(judgements, proposed) : unread;
    field("proposals").replaceChildren(...made.map(showProposal(made.length)));
    field("more").hidden = made.length < 2;
  };
  await fill();
  document.addEventListener("input", fill);
  status.textContent = `Probed. Check each row, then propose the results on GitHub.${missing}`;
  await tell(status.textContent);
};

// Offer the run's submitters to the server that serves the page. Every set is submitted in turn, the button off
// meanwhile so a second press cannot send them twice, and how each went is written to the status. It opens
// "Submitted." only when every set went in or was there already, and there was one to send: that is what a driver
// reading it is told the run succeeded by.
const offer = async (submitters, missing) => {
  const submitAll = async () => {
    const button = field("submit-all");
    button.disabled = true;
    status.className = "";
    const outcomes = [];
    for (const shown of submitters) {
      status.textContent = `Submitting ${outcomes.length + 1} of ${submitters.length} probe sets...`;
      outcomes.push(await shown.submit());
    }
    const lines = outcomes.map((outcome) => outcome.line).join("\n");
    const taken = outcomes.length > 0 && outcomes.every((outcome) => outcome.kind !== "bad");
    const verdict = taken ? "Submitted." : "Not submitted.";
    status.className = taken ? "" : "bad";
    status.textContent = `${verdict}${missing}\n${lines}`;
    button.disabled = false;
    await tell(status.textContent);
  };
  // The button stays off until every probe set has been measured, which is only now.
  field("submit-all").disabled = submitters.length === 0;
  field("submit-all").addEventListener("click", submitAll);
  status.textContent = `Probed. Check each row, then submit.${missing}`;
  if (query.has("submit")) await submitAll();
};

// Say the run failed, and why. The server is told the browser and where the error arose too, since that is what
// finding its cause needs, and a button sends it again, saying whether it arrived, for a phone whose first post was
// lost.
const fail = async (error) => {
  status.className = "bad";
  status.textContent = `The probes could not run: ${String(error)}`;
  const stack = error?.stack ? [`stack: ${error.stack}`] : [];
  const lines = [status.textContent, `user agent: ${navigator.userAgent}`, ...stack];
  const report = lines.join("\n");
  const told = await tell(report);
  if (!served) return;
  const button = document.createElement("button");
  button.textContent = "Send this error to the server";
  const outcome = document.createElement("p");
  outcome.textContent = told ? "The server has been told." : "";
  button.addEventListener("click", async () => {
    button.disabled = true;
    outcome.textContent = "Sending...";
    const sent = await tell(report);
    outcome.textContent = sent ? "Sent: the server has logged it." : "Not sent: the server did not answer.";
    button.disabled = false;
  });
  status.after(button, outcome);
};

// The run, in a function rather than at the module's top level: the legacy build bundles this module into a classic
// script, which cannot await at its top level.
const run = async () => {
  served = await fetch("/signatures.yaml").then(
    (response) => response.ok,
    () => false,
  );
  try {
    probes = await loadProbes();
    await fillShared();
    status.textContent = "Running the probes...";
    const { submitters, problems } = await probeAll();
    const missing = problems.length ? ` Not probed: ${problems.join("; ")}.` : "";
    await (served ? offer(submitters, missing) : propose(submitters, missing));
  } catch (error) {
    await fail(error);
  }
};

void run();
