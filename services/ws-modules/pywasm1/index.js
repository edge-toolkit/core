import init, { pyExec } from "./rustpython_wasm.js";

export async function run() {
  const src = await fetch(new URL("./main.py", import.meta.url)).then((r) => r.text());
  await init();
  pyExec(src, { stdout: (msg) => console.log(msg) });
}
