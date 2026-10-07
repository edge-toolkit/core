// edge_toolkit.js -- installs the edge-toolkit wheel, and the et-org and et-ws wheels it imports, into a
// Pyodide-based ws-module, so a consumer makes one call for all three.

// skipcq: JS-0833 -- committed ES module; the analyzer's script-mode parse is a false positive
const { installWheel: installEtOrg, installWheelFrom } = await import("/modules/@edge-toolkit/et-org/et_org.js");
const { installWheel: installEtWs } = await import("/modules/@edge-toolkit/et-ws/et_ws.js");

export async function installWheel(pyodide) {
  await installEtOrg(pyodide);
  await installEtWs(pyodide);
  await installWheelFrom(pyodide, import.meta.url);
}
