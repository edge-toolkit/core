package au.edu.curtin.et;

import org.teavm.jso.JSBody;
import org.teavm.jso.JSExport;
import org.teavm.jso.JSObject;
import org.teavm.jso.core.JSPromise;
import org.teavm.jso.function.JSConsumer;

public final class Data1 {

    @JSBody(params = {"msg"}, script = "host.log(msg);")
    static native void log(String msg);

    @JSBody(params = {"msg"}, script = "host.setStatus(msg);")
    static native void setStatus(String msg);

    @JSBody(script = "return host.getWsUrl();")
    static native String getWsUrl();

    @JSBody(params = {"url"}, script = "host.wsConnect(url);")
    static native void wsConnect(String url);

    @JSBody(script = "host.wsDisconnect();")
    static native void wsDisconnect();

    @JSBody(script = "return host.wsGetState();")
    static native String wsGetState();

    @JSBody(script = "return host.wsGetAgentId();")
    static native String wsGetAgentId();

    @JSBody(params = {"ms"}, script = "return host.sleep(ms);")
    static native JSPromise<JSObject> sleep(int ms);

    @JSBody(params = {"url", "body"}, script = "return host.putFile(url, body);")
    static native JSPromise<JSObject> putFile(String url, String body);

    @JSBody(params = {"url"}, script = "return host.getFile(url);")
    static native JSPromise<JSObject> getFile(String url);

    @JSBody(script = "return new Date().toISOString();")
    static native String getIsoTimestamp();

    @JSBody(params = {"msg"}, script = "return new Error(msg);")
    static native JSObject jsError(String msg);

    @JSBody(params = {"obj"}, script = "return String(obj);")
    static native String jsObjectToString(JSObject obj);

    @JSExport
    public static JSPromise<JSObject> run() {
        return new JSPromise<>((resolve, reject) -> runAsync(resolve, reject));
    }

    private static void runAsync(JSConsumer<JSObject> resolve, JSConsumer<Object> reject) {
        log("[java-data1] entered run()");
        setStatus("[java-data1] entered run()");
        wsConnect(getWsUrl());
        waitForConnected(0, resolve, reject);
    }

    private static void waitForConnected(int attempt, JSConsumer<JSObject> resolve, JSConsumer<Object> reject) {
        if (attempt >= 100) {
            reject.accept(jsError("Timeout waiting for WebSocket connection"));
            return;
        }
        if ("connected".equals(wsGetState())) {
            waitForAgentId(0, resolve, reject);
            return;
        }
        sleep(100).then(v -> {
            waitForConnected(attempt + 1, resolve, reject);
            return null;
        });
    }

    private static void waitForAgentId(int attempt, JSConsumer<JSObject> resolve, JSConsumer<Object> reject) {
        if (attempt >= 100) {
            reject.accept(jsError("Timeout waiting for agent_id"));
            return;
        }
        String agentId = wsGetAgentId();
        if (agentId != null && !agentId.isEmpty()) {
            String msg = "[java-data1] connected as " + agentId;
            log(msg);
            setStatus(msg);
            doStore(agentId, resolve, reject);
            return;
        }
        sleep(100).then(v -> {
            waitForAgentId(attempt + 1, resolve, reject);
            return null;
        });
    }

    private static void doStore(String agentId, JSConsumer<JSObject> resolve, JSConsumer<Object> reject) {
        String filename = "test_data.txt";
        String testContent = "Hello from java-data1 at " + getIsoTimestamp() + "!";
        String storageUrl = "/storage/" + agentId + "/" + filename;
        String msg = "[java-data1] storing data to " + storageUrl;
        log(msg);
        setStatus(msg);
        putFile(storageUrl, testContent).then(v -> {
            doFetch(storageUrl, testContent, resolve, reject);
            return null;
        });
    }

    private static void doFetch(
            String storageUrl, String testContent, JSConsumer<JSObject> resolve, JSConsumer<Object> reject) {
        String msg = "[java-data1] fetching data from " + storageUrl;
        log(msg);
        setStatus(msg);
        getFile(storageUrl).then(result -> {
            verifyAndFinish(testContent, jsObjectToString(result), resolve, reject);
            return null;
        });
    }

    private static void verifyAndFinish(
            String testContent, String retrieved, JSConsumer<JSObject> resolve, JSConsumer<Object> reject) {
        if (testContent.equals(retrieved)) {
            String ok = "[java-data1] VERIFICATION SUCCESS - data matches!";
            log(ok);
            setStatus(ok);
        } else {
            String fail = "[java-data1] VERIFICATION FAILURE\nSent: " + testContent + "\nGot: " + retrieved;
            log(fail);
            setStatus(fail);
            reject.accept(jsError("Data mismatch"));
            return;
        }
        sleep(2000).then(v -> {
            wsDisconnect();
            String done = "[java-data1] workflow complete";
            log(done);
            setStatus(done);
            resolve.accept(null);
            return null;
        });
    }
}
