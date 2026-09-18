// Browser launcher for the loopback-only MLflow server used by the vessel FL demo.

function mlflowUrl() {
  return String(globalThis.__VESSEL_FL_MLFLOW_URL || "http://127.0.0.1:5000/");
}

function show(message) {
  const output = document.getElementById("module-output");
  if (output) {
    output.value = `[vessel MLflow] ${message}`;
    output.scrollTop = output.scrollHeight;
  }
  console.log(`[vessel MLflow] ${message}`);
}

export default async function init() {
  show(`ready; Run opens ${mlflowUrl()}`);
}

export function run() {
  const url = mlflowUrl();
  window.open(url, "_blank", "noopener,noreferrer");
  show(`opened ${url}`);
}

export function stop() {}

export const is_running = () => false;
