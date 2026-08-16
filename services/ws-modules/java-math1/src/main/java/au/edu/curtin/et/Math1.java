package au.edu.curtin.et;

import org.teavm.jso.JSBody;
import org.teavm.jso.JSExport;
import org.teavm.jso.JSObject;
import org.teavm.jso.core.JSPromise;
import org.teavm.jso.function.JSConsumer;

public final class Math1 {

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

    @JSBody(script = "return host.hasInput();")
    static native boolean hasInput();

    @JSBody(script = "return host.loadInput();")
    static native JSPromise<JSObject> loadInput();

    @JSBody(script = "return host.inputClientCount();")
    static native int inputClientCount();

    @JSBody(params = {"client"}, script = "return host.inputSampleCount(client);")
    static native int inputSampleCount(int client);

    @JSBody(params = {"client", "index"}, script = "return host.inputFeature(client, index);")
    static native double inputFeature(int client, int index);

    @JSBody(params = {"client", "index"}, script = "return host.inputTarget(client, index);")
    static native double inputTarget(int client, int index);

    @JSBody(script = "return host.inputRounds();")
    static native int inputRounds();

    @JSBody(script = "return host.inputEpochs();")
    static native int inputEpochs();

    @JSBody(script = "return host.inputLearningRate();")
    static native double inputLearningRate();

    @JSBody(script = "return host.inputDescribe();")
    static native String inputDescribe();

    @JSBody(params = {"module", "weight", "bias"}, script = "return host.putOutput(module, weight, bias);")
    static native JSPromise<JSObject> putOutput(String module, double weight, double bias);

    @JSBody(params = {"msg"}, script = "return new Error(msg);")
    static native JSObject jsError(String msg);

    private Math1() {}

    @JSExport
    public static JSPromise<JSObject> run() {
        return new JSPromise<>((resolve, reject) -> runAsync(resolve, reject));
    }

    private static void status(String msg) {
        log("[java-math1] " + msg);
        setStatus("[java-math1] " + msg);
    }

    private static void runAsync(JSConsumer<JSObject> resolve, JSConsumer<Object> reject) {
        status("entered run()");
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
            status("connected as " + agentId);
            status("waiting for the math1-input pointer broadcast");
            waitForInput(0, resolve, reject);
            return;
        }
        sleep(100).then(v -> {
            waitForAgentId(attempt + 1, resolve, reject);
            return null;
        });
    }

    private static void waitForInput(int attempt, JSConsumer<JSObject> resolve, JSConsumer<Object> reject) {
        if (attempt >= 100) {
            reject.accept(jsError("Timeout waiting for the math1-input pointer"));
            return;
        }
        if (hasInput()) {
            loadInput().then(v -> {
                computeAndStore(resolve, reject);
                return null;
            });
            return;
        }
        sleep(100).then(v -> {
            waitForInput(attempt + 1, resolve, reject);
            return null;
        });
    }

    /**
     * Runs the FedAvg simulation over the host-parsed input and returns the global {weight, bias}.
     *
     * <p>Only + - * / on double in a fixed evaluation order, so the result is bit-identical to the
     * other math1 language twins. The input is read through the shim's typed accessors because the
     * TeaVM guest carries no JSON parser of its own.
     */
    private static double[] fedAvg() {
        int rounds = inputRounds();
        int epochs = inputEpochs();
        double learningRate = inputLearningRate();
        int clientCount = inputClientCount();
        double weight = 0.0;
        double bias = 0.0;
        double totalSamples = 0.0;
        for (int client = 0; client < clientCount; client++) {
            totalSamples += inputSampleCount(client);
        }
        for (int round = 0; round < rounds; round++) {
            double mergedWeight = 0.0;
            double mergedBias = 0.0;
            for (int client = 0; client < clientCount; client++) {
                int sampleCount = inputSampleCount(client);
                double count = sampleCount;
                double clientWeight = weight;
                double clientBias = bias;
                for (int epoch = 0; epoch < epochs; epoch++) {
                    double gradWeight = 0.0;
                    double gradBias = 0.0;
                    for (int index = 0; index < sampleCount; index++) {
                        double feature = inputFeature(client, index);
                        double target = inputTarget(client, index);
                        double residual = clientWeight * feature + clientBias - target;
                        gradWeight += residual * feature;
                        gradBias += residual;
                    }
                    clientWeight -= learningRate * (2.0 * gradWeight / count);
                    clientBias -= learningRate * (2.0 * gradBias / count);
                }
                mergedWeight += clientWeight * count;
                mergedBias += clientBias * count;
            }
            weight = mergedWeight / totalSamples;
            bias = mergedBias / totalSamples;
        }
        return new double[] {weight, bias};
    }

    private static void computeAndStore(JSConsumer<JSObject> resolve, JSConsumer<Object> reject) {
        status("running FedAvg - " + inputDescribe());
        double[] model = fedAvg();
        double weight = model[0];
        double bias = model[1];
        status("global model weight=" + weight + " bias=" + bias);
        putOutput("java-math1", weight, bias).then(v -> {
            status("stored the global model to math1-output.json");
            finish(resolve);
            return null;
        });
    }

    private static void finish(JSConsumer<JSObject> resolve) {
        sleep(2000).then(v -> {
            wsDisconnect();
            status("workflow complete");
            resolve.accept(null);
            return null;
        });
    }
}
