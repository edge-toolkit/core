// Pyodide coverage collection, active only under the web-runner when ET_TEST_COVERAGE is set.
// The runner surfaces that env var as globalThis.__ET_TEST_COVERAGE (see runtime.rs); when it is present this
// fragment defines globalThis.__etPyCov, a {start, stop} pair the Pyodide module shims call around their
// pyimport + run(). coverage.py ships in the pinned Pyodide distribution (a coverage wasm wheel under
// /modules/pyodide/), so loadPackage pulls it with no network. start() begins tracing before the module is
// imported; stop() writes the .coverage data file and PUTs it to ws-server storage at /storage/pycov/<pkg>.coverage,
// where the web-runner integration test collects it for the combined Python coverage report. In a browser (no
// __ET_TEST_COVERAGE) nothing here runs, so shipped modules are unaffected.
if (globalThis.__ET_TEST_COVERAGE) {
  globalThis.__etPyCov = {
    // Load coverage.py from the local Pyodide dist and start tracing `pkg` before it is imported, so
    // import-time lines are captured too. Runs synchronously in the Pyodide interpreter.
    async start(pyodide, pkg) {
      await pyodide.loadPackage("coverage");
      pyodide.runPython(`
import coverage as _et_cov_mod
_et_cov = _et_cov_mod.Coverage(data_file="/tmp/${pkg}.coverage", source=["${pkg}"])
_et_cov.start()
`);
    },
    // Stop tracing, persist the .coverage data file, and PUT it to ws-server storage for the test to collect.
    // A collection failure propagates and fails the coverage run -- capturing coverage is that run's purpose.
    async stop(pyodide, pkg) {
      pyodide.runPython("_et_cov.stop()\n_et_cov.save()\n");
      const data = pyodide.FS.readFile(`/tmp/${pkg}.coverage`);
      const base = typeof globalThis.__ET_HTTP_BASE === "string" ? globalThis.__ET_HTTP_BASE : "";
      await fetch(`${base}/storage/pycov/${pkg}.coverage`, { method: "PUT", body: data });
    },
  };
}
