// Kotlin 2.x still stability-gates the whole wasmJs interop surface (js(), JsAny, Promise) behind this
// opt-in; every declaration in this file is an interop bridge, so the opt-in is file-scoped.
@file:OptIn(ExperimentalWasmJsInterop::class)

package au.edu.curtin.et

import kotlin.js.ExperimentalWasmJsInterop
import kotlin.js.Promise

// The pkg/ shim installs a `host` global carrying the browser-side WebSocket/storage/console operations, the
// same contract java-data1's TeaVM @JSBody bindings use. Each js() body below is a single-expression bridge.
private fun hostLog(msg: String): Unit = js("host.log(msg)")

private fun hostSetStatus(msg: String): Unit = js("host.setStatus(msg)")

private fun hostGetWsUrl(): String = js("host.getWsUrl()")

private fun hostWsConnect(url: String): Unit = js("host.wsConnect(url)")

private fun hostWsDisconnect(): Unit = js("host.wsDisconnect()")

private fun hostWsGetState(): String = js("host.wsGetState()")

private fun hostWsGetAgentId(): String = js("host.wsGetAgentId()")

private fun hostSleep(ms: Int): Promise<JsAny?> = js("host.sleep(ms)")

private fun hostPutFile(url: String, body: String): Promise<JsAny?> = js("host.putFile(url, body)")

private fun hostGetFile(url: String): Promise<JsAny?> = js("host.getFile(url)")

private fun isoTimestamp(): String = js("new Date().toISOString()")

private fun jsToString(value: JsAny?): String = js("String(value)")

private fun resolvedPromise(): Promise<JsAny?> = js("Promise.resolve(null)")

private fun rejectedPromise(msg: String): Promise<JsAny?> = js("Promise.reject(new Error(msg))")

private fun installRun(f: () -> Promise<JsAny?>): Unit = js("globalThis.kotlinData1Run = f")

private fun status(msg: String) {
    hostLog("[kotlin-data1] $msg")
    hostSetStatus("[kotlin-data1] $msg")
}

private fun waitUntil(what: String, attempt: Int = 0, ready: () -> Boolean): Promise<JsAny?> = when {
    ready() -> resolvedPromise()
    attempt >= 100 -> rejectedPromise("Timeout waiting for $what")
    else -> hostSleep(100).then<JsAny?> { waitUntil(what, attempt + 1, ready) }
}

private fun storeAndVerify(agentId: String): Promise<JsAny?> {
    val content = "Hello from kotlin-data1 at ${isoTimestamp()}!"
    val storageUrl = "/storage/$agentId/test_data.txt"
    status("storing data to $storageUrl")
    return hostPutFile(storageUrl, content)
        .then<JsAny?> {
            status("fetching data from $storageUrl")
            hostGetFile(storageUrl)
        }
        .then<JsAny?> { retrieved ->
            val got = jsToString(retrieved)
            if (got == content) {
                status("VERIFICATION SUCCESS - data matches!")
                null
            } else {
                status("VERIFICATION FAILURE - data mismatch!\nSent: $content\nGot: $got")
                rejectedPromise("Data mismatch")
            }
        }
}

private fun runWorkflow(): Promise<JsAny?> {
    status("entered run()")
    hostWsConnect(hostGetWsUrl())
    return waitUntil("WebSocket connection") { hostWsGetState() == "connected" }
        .then<JsAny?> { waitUntil("agent_id") { hostWsGetAgentId().isNotEmpty() } }
        .then<JsAny?> {
            val agentId = hostWsGetAgentId()
            status("connected as $agentId")
            storeAndVerify(agentId)
        }
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
