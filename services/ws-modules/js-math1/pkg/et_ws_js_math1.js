// et_ws_js_math1.js -- math1 twin in plain JavaScript (no bundler, no dependencies).
//
// Storage-driven FedAvg: waits for the broadcast math1-input pointer, reads the input JSON
// (client datasets + hyperparameters) from ws-server storage, runs the kernel -- only + - * / on
// IEEE-754 doubles, bit-identical to the other math1 twins -- and stores the global model to
// math1-output.json in its own bucket, where the test harness reads and verifies it.

function appendOutput(msg) {
  const el = document.getElementById("module-output");
  if (el) {
    el.value = el.value ? `${el.value}\n${msg}` : msg;
  }
}

function log(msg) {
  const line = `[js-math1] ${msg}`;
  console.log(line);
  appendOutput(line);
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function waitFor(what, ready) {
  for (let attempt = 0; attempt < 100; attempt++) {
    const value = ready();
    if (value) {
      return value;
    }
    await sleep(100);
  }
  throw new Error(`Timeout waiting for ${what}`);
}

function fedAvg(input) {
  let weight = 0.0;
  let bias = 0.0;
  let totalSamples = 0.0;
  for (const samples of input.clients) {
    totalSamples += samples.length;
  }
  for (let round = 0; round < input.rounds; round++) {
    let mergedWeight = 0.0;
    let mergedBias = 0.0;
    for (const samples of input.clients) {
      const count = samples.length;
      let clientWeight = weight;
      let clientBias = bias;
      for (let epoch = 0; epoch < input.epochs; epoch++) {
        let gradWeight = 0.0;
        let gradBias = 0.0;
        for (const [feature, target] of samples) {
          const residual = clientWeight * feature + clientBias - target;
          gradWeight += residual * feature;
          gradBias += residual;
        }
        clientWeight -= input.learning_rate * ((2.0 * gradWeight) / count);
        clientBias -= input.learning_rate * ((2.0 * gradBias) / count);
      }
      mergedWeight += clientWeight * count;
      mergedBias += clientBias * count;
    }
    weight = mergedWeight / totalSamples;
    bias = mergedBias / totalSamples;
  }
  return [weight, bias];
}

export default async function init() {}

export async function run() {
  log("entered run()");

  const wasmAgent = await import("/modules/et-ws-wasm-agent/et_ws_wasm_agent.js");
  await wasmAgent.default();
  const proto = window.location.protocol === "https:" ? "wss:" : "ws:";
  const wsUrl = `${proto}//${window.location.host}/ws`;
  const client = new wasmAgent.WsClient(new wasmAgent.WsClientConfig(wsUrl));

  let pointer = null;
  client.set_on_message((frame) => {
    if (typeof frame !== "string") return;
    try {
      const msg = JSON.parse(frame);
      if (msg.type === "math1-input" && msg.bucket && msg.filename) pointer = msg;
    } catch {}
  });

  client.connect();
  await waitFor("WebSocket connection", () => client.get_state() === "connected");
  const agentId = await waitFor("agent_id", () => client.get_agent_id());
  log(`connected as ${agentId}`);

  log("waiting for the math1-input pointer broadcast");
  const input_ptr = await waitFor("math1-input pointer", () => pointer);
  log(`reading input from /storage/${input_ptr.bucket}/${input_ptr.filename}`);
  const inputResponse = await fetch(`/storage/${input_ptr.bucket}/${input_ptr.filename}`);
  if (!inputResponse.ok) throw new Error(`input GET failed: ${inputResponse.status}`);
  const input = await inputResponse.json();

  log(`running FedAvg - ${input.clients.length} clients x ${input.rounds} rounds x ${input.epochs} local epochs`);
  const [weight, bias] = fedAvg(input);
  log(`global model weight=${weight} bias=${bias}`);

  const output = JSON.stringify({ module: "js-math1", weight, bias });
  const putResponse = await fetch(`/storage/${agentId}/math1-output.json`, { method: "PUT", body: output });
  if (!putResponse.ok) throw new Error(`output PUT failed: ${putResponse.status}`);
  log(`stored the global model to /storage/${agentId}/math1-output.json`);

  await sleep(2000);
  client.disconnect();
  log("workflow complete");
}
