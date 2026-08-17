// Kotlin 2.x still stability-gates the whole wasmJs interop surface (js(), JsAny, Promise) behind this
// opt-in; every declaration in this file is an interop bridge, so the opt-in is file-scoped.
@file:OptIn(ExperimentalWasmJsInterop::class)

package au.edu.curtin.et

import kotlin.js.ExperimentalWasmJsInterop
import kotlin.js.Promise

// The pkg/ shim installs a `host` global carrying the browser-side WebSocket, storage, and parsed
// math1-input accessors; each js() body below is a single-expression bridge. The kernel reads the
// input through the typed accessors because the WasmGC guest carries no JSON parser of its own.
private fun hostLog(msg: String): Unit = js("host.log(msg)")

private fun hostSetStatus(msg: String): Unit = js("host.setStatus(msg)")

private fun hostGetWsUrl(): String = js("host.getWsUrl()")

private fun hostWsConnect(url: String): Unit = js("host.wsConnect(url)")

private fun hostWsDisconnect(): Unit = js("host.wsDisconnect()")

private fun hostWsGetState(): String = js("host.wsGetState()")

private fun hostWsGetAgentId(): String = js("host.wsGetAgentId()")

private fun hostSleep(ms: Int): Promise<JsAny?> = js("host.sleep(ms)")

private fun hostHasInput(): Boolean = js("host.hasInput()")

private fun hostLoadInput(): Promise<JsAny?> = js("host.loadInput()")

private fun hostInputClientCount(): Int = js("host.inputClientCount()")

private fun hostInputSampleCount(client: Int): Int = js("host.inputSampleCount(client)")

private fun hostInputFeature(client: Int, index: Int): Double = js("host.inputFeature(client, index)")

private fun hostInputTarget(client: Int, index: Int): Double = js("host.inputTarget(client, index)")

private fun hostInputRounds(): Int = js("host.inputRounds()")

private fun hostInputEpochs(): Int = js("host.inputEpochs()")

private fun hostInputLearningRate(): Double = js("host.inputLearningRate()")

private fun hostInputDescribe(): String = js("host.inputDescribe()")

private fun hostPutOutput(module: String, weight: Double, bias: Double): Promise<JsAny?> =
    js("host.putOutput(module, weight, bias)")

private fun resolvedPromise(): Promise<JsAny?> = js("Promise.resolve(null)")

private fun rejectedPromise(msg: String): Promise<JsAny?> = js("Promise.reject(new Error(msg))")

private fun installRun(f: () -> Promise<JsAny?>): Unit = js("globalThis.kotlinMath1Run = f")

private fun status(msg: String) {
    hostLog("[kotlin-math1] $msg")
    hostSetStatus("[kotlin-math1] $msg")
}

private fun waitUntil(what: String, attempt: Int = 0, ready: () -> Boolean): Promise<JsAny?> = when {
    ready() -> resolvedPromise()
    attempt >= 100 -> rejectedPromise("Timeout waiting for $what")
    else -> hostSleep(100).then<JsAny?> { waitUntil(what, attempt + 1, ready) }
}

// Runs the FedAvg simulation over the host-parsed input and returns the final global (weight, bias).
// Only + - * / on Double in a fixed evaluation order, so the result is bit-identical to the other
// math1 language twins.
private fun fedAvg(): Pair<Double, Double> {
    val rounds = hostInputRounds()
    val epochs = hostInputEpochs()
    val learningRate = hostInputLearningRate()
    val clientCount = hostInputClientCount()
    var weight = 0.0
    var bias = 0.0
    var totalSamples = 0.0
    for (client in 0 until clientCount) totalSamples += hostInputSampleCount(client).toDouble()
    repeat(rounds) {
        var mergedWeight = 0.0
        var mergedBias = 0.0
        for (client in 0 until clientCount) {
            val sampleCount = hostInputSampleCount(client)
            val count = sampleCount.toDouble()
            var clientWeight = weight
            var clientBias = bias
            repeat(epochs) {
                var gradWeight = 0.0
                var gradBias = 0.0
                for (index in 0 until sampleCount) {
                    val feature = hostInputFeature(client, index)
                    val target = hostInputTarget(client, index)
                    val residual = clientWeight * feature + clientBias - target
                    gradWeight += residual * feature
                    gradBias += residual
                }
                clientWeight -= learningRate * (2.0 * gradWeight / count)
                clientBias -= learningRate * (2.0 * gradBias / count)
            }
            mergedWeight += clientWeight * count
            mergedBias += clientBias * count
        }
        weight = mergedWeight / totalSamples
        bias = mergedBias / totalSamples
    }
    return weight to bias
}

private fun computeAndStore(): Promise<JsAny?> {
    status("running FedAvg - ${hostInputDescribe()}")
    val (weight, bias) = fedAvg()
    status("global model weight=$weight bias=$bias")
    return hostPutOutput("kotlin-math1", weight, bias).then<JsAny?> {
        status("stored the global model to math1-output.json")
        null
    }
}

private fun runWorkflow(): Promise<JsAny?> {
    status("entered run()")
    hostWsConnect(hostGetWsUrl())
    return waitUntil("WebSocket connection") { hostWsGetState() == "connected" }
        .then<JsAny?> { waitUntil("agent_id") { hostWsGetAgentId().isNotEmpty() } }
        .then<JsAny?> {
            status("connected as ${hostWsGetAgentId()}")
            status("waiting for the math1-input pointer broadcast")
            waitUntil("math1-input pointer") { hostHasInput() }
        }
        .then<JsAny?> { hostLoadInput() }
        .then<JsAny?> { computeAndStore() }
        .then<JsAny?> { hostSleep(2000) }
        .then<JsAny?> {
            hostWsDisconnect()
            status("workflow complete")
            null
        }
}

fun main() {
    installRun { runWorkflow() }
}
