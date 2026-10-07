// et_org.js -- installs the et-org wheel into a Pyodide-based ws-module, and exports the install routine the other
// Python library ws-modules reuse.

// Fetch the wheel a ws-module ships beside `moduleUrl` and put it on `pyodide`'s path. The wheel is found by the
// `<name>-<version>-py3-none-any.whl` convention from that module's own package.json, so a version bump never
// touches a consumer.
// skipcq: JS-0833 -- committed ES module; the analyzer's script-mode parse is a false positive
export async function installWheelFrom(pyodide, moduleUrl) {
  const pkg = await fetch(new URL("package.json", moduleUrl)).then((r) => r.json());
  const distribution = pkg.name.split("/").pop();
  const wheel = `${distribution.replace(/-/g, "_")}-${pkg.version}-py3-none-any.whl`;
  const bytes = new Uint8Array(await fetch(new URL(wheel, moduleUrl)).then((r) => r.arrayBuffer()));
  pyodide.FS.writeFile(`/tmp/${wheel}`, bytes);
  pyodide.runPython(`import sys\nsys.path.insert(0, "/tmp/${wheel}")`);
}

export function installWheel(pyodide) {
  return installWheelFrom(pyodide, import.meta.url);
}
