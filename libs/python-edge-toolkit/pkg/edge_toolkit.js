// edge_toolkit.js -- the start-up every Pyodide-based ws-module shares: loading Pyodide from the distribution served
// at /modules/pyodide/, and installing the edge-toolkit wheel with the et-org and et-ws wheels it imports.

const PYODIDE_BASE_PATH = "/modules/pyodide/";

// The page's one load of Pyodide's script, shared by every module on it so none appends the script twice. A failed
// load clears it again, so the next caller retries rather than inheriting the rejection.
let pyodideScript = null;

// Load Pyodide's script, which defines `globalThis.loadPyodide`.
// skipcq: JS-0833 -- committed ES module; the analyzer's script-mode parse is a false positive
export async function loadPyodideScript() {
  if (globalThis.loadPyodide) return;
  pyodideScript ??= new Promise((resolve, reject) => {
    const script = document.createElement("script");
    script.src = `${PYODIDE_BASE_PATH}pyodide.js`;
    // eslint-disable-next-line unicorn/prefer-add-event-listener -- et-ws-web-runner's document stub calls only onload
    script.onload = resolve;
    // eslint-disable-next-line unicorn/prefer-add-event-listener -- et-ws-web-runner's document stub calls only onerror
    script.onerror = reject;
    document.head.appendChild(script);
  });
  try {
    await pyodideScript;
  } catch (error) {
    pyodideScript = null;
    throw error;
  }
}

// Start a Pyodide interpreter whose packages resolve from the same origin, so nothing is fetched from a CDN.
export async function startPyodide() {
  await loadPyodideScript();
  return globalThis.loadPyodide({ indexURL: PYODIDE_BASE_PATH });
}

export async function installWheel(pyodide) {
  const { installWheel: installEtOrg, installWheelFrom } = await import("/modules/@edge-toolkit/et-org/et_org.js");
  const { installWheel: installEtWs } = await import("/modules/@edge-toolkit/et-ws/et_ws.js");
  await installEtOrg(pyodide);
  await installEtWs(pyodide);
  await installWheelFrom(pyodide, import.meta.url);
}
