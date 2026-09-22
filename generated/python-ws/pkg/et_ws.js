// et_ws.js — helper exposed to Pyodide-based ws-modules that need the
// generated Pydantic models. Mounted at /modules/@edge-toolkit/et-ws/ by et-modules-service
// (each consumer declares `et-ws = "*"` in `[tool.ws-module.dependencies]`).
//
// The wheel ships next to this file in pkg/. Consumers import `installWheel`
// and call it with their `pyodide` instance; we fetch the wheel by the
// `<name>-<version>-py3-none-any.whl` convention so version bumps don't
// require touching every consumer.

// skipcq: JS-0833 -- committed ES module; the analyzer's script-mode parse is a false positive
export async function installWheel(pyodide) {
  const pkg = await fetch(new URL("package.json", import.meta.url)).then((r) => r.json());
  const distribution = pkg.name.split("/").pop();
  const wheel = `${distribution.replace(/-/g, "_")}-${pkg.version}-py3-none-any.whl`;
  const bytes = new Uint8Array(await fetch(new URL(wheel, import.meta.url)).then((r) => r.arrayBuffer()));
  pyodide.FS.writeFile(`/tmp/${wheel}`, bytes);
  pyodide.runPython(`import sys\nsys.path.insert(0, "/tmp/${wheel}")`);
}
