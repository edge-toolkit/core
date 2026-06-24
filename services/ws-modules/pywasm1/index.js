import init, { vmStore } from "./rustpython_wasm.js";

// cowsay (the mise `pipx:cowsay` package) ships as plain modules next to main.py. rustpython-WASM has no
// filesystem for third-party packages, so inject each module into the VM before running main.py. __package__ is
// set so cowsay's intra-package relative imports (`from .main import ...`) resolve; submodules are injected first.
const COWSAY_MODULES = [
  ["cowsay.characters", "./cowsay/characters.py"],
  ["cowsay.main", "./cowsay/main.py"],
  ["cowsay", "./cowsay/__init__.py"],
];

export async function run() {
  await init();
  const fetchText = (path) => fetch(new URL(path, import.meta.url)).then((r) => r.text());
  const vm = vmStore.init("et-ws-pywasm1");
  vm.setStdout((msg) => console.log(msg));
  for (const [name, path] of COWSAY_MODULES) {
    vm.injectModule(name, await fetchText(path), { __package__: "cowsay" });
  }
  vm.exec(await fetchText("./main.py"));
}
