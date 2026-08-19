import 'dart:async';
import 'dart:convert';
import 'dart:js_interop';
import 'dart:typed_data';

import 'package:dio/dio.dart';
import 'package:et_rest/export.dart';

// JS interop declarations for et_ws_wasm_agent
@JS()
extension type WsClientConfig._(JSObject _) implements JSObject {
  external factory WsClientConfig(String serverUrl);
}

@JS()
extension type WsClient._(JSObject _) implements JSObject {
  external factory WsClient(WsClientConfig config);
  external void connect();
  external void disconnect();
  // ignore: non_constant_identifier_names
  external String get_state();
  // ignore: non_constant_identifier_names
  external String get_agent_id();
  // ignore: non_constant_identifier_names
  external void set_on_message(JSFunction callback);
}

// JS interop for browser globals
@JS('window.location.protocol')
external String get locationProtocol;

@JS('window.location.host')
external String get locationHost;

@JS('document.getElementById')
external JSObject? getElementById(String id);

@JS()
extension type _TextArea._(JSObject _) implements JSObject {
  external String get value;
  external set value(String v);
}

void appendOutput(String msg) {
  final el = getElementById('module-output');
  if (el != null) {
    final ta = el as _TextArea;
    ta.value = ta.value.isEmpty ? msg : '${ta.value}\n$msg';
  }
}

void log(String msg) {
  appendOutput('[dart-math1] $msg');
}

String get wsUrl {
  final proto = locationProtocol == 'https:' ? 'wss:' : 'ws:';
  return '$proto//$locationHost/ws';
}

Future<void> sleep(int ms) {
  final c = Completer<void>();
  Timer(Duration(milliseconds: ms), c.complete);
  return c.future;
}

Future<T> waitFor<T>(String what, T? Function() ready) async {
  for (var i = 0; i < 100; i++) {
    final value = ready();
    if (value != null) return value;
    await sleep(100);
  }
  throw Exception('Timeout waiting for $what');
}

/// The broadcast pointer naming the storage bucket + filename of the input JSON.
({String bucket, String filename})? inputPointer;

/// One path segment of a storage URL, with no separator or traversal syntax.
///
/// The pointer arrives in a relayed broadcast from an arbitrary peer, so a `bucket` of `../..` would
/// otherwise steer the read at another storage path. The java, kotlin, and dotnet twins hold their pointers
/// to the same shape, so every math1 twin shares one trust model.
final _safeSegment = RegExp(r'^[A-Za-z0-9][A-Za-z0-9._-]*$');

void captureInputPointer(String frame) {
  try {
    final msg = jsonDecode(frame);
    if (msg is Map<String, dynamic> &&
        msg['type'] == 'math1-input' &&
        msg['bucket'] is String &&
        msg['filename'] is String &&
        _safeSegment.hasMatch(msg['bucket'] as String) &&
        _safeSegment.hasMatch(msg['filename'] as String)) {
      inputPointer = (
        bucket: msg['bucket'] as String,
        filename: msg['filename'] as String,
      );
    }
  } on FormatException {
    // Not JSON -- some other relayed frame; ignore.
  }
}

/// Runs the FedAvg simulation on the fetched input and returns the final global [weight, bias].
///
/// Only + - * / on doubles, in a fixed evaluation order, so the result is bit-identical to the
/// other math1 language twins.
List<double> fedAvg(Map<String, dynamic> input) {
  final clients = (input['clients'] as List)
      .map(
        (samples) => (samples as List)
            .map(
              (sample) =>
                  (sample as List).map((v) => (v as num).toDouble()).toList(),
            )
            .toList(),
      )
      .toList();
  final rounds = input['rounds'] as int;
  final epochs = input['epochs'] as int;
  final learningRate = (input['learning_rate'] as num).toDouble();

  var weight = 0.0;
  var bias = 0.0;
  var totalSamples = 0.0;
  for (final samples in clients) {
    totalSamples += samples.length.toDouble();
  }
  for (var round = 0; round < rounds; round++) {
    var mergedWeight = 0.0;
    var mergedBias = 0.0;
    for (final samples in clients) {
      final count = samples.length.toDouble();
      var clientWeight = weight;
      var clientBias = bias;
      for (var epoch = 0; epoch < epochs; epoch++) {
        var gradWeight = 0.0;
        var gradBias = 0.0;
        for (final sample in samples) {
          final residual = clientWeight * sample[0] + clientBias - sample[1];
          gradWeight += residual * sample[0];
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
  return [weight, bias];
}

Future<void> run() async {
  log('entered run()');

  final client = WsClient(WsClientConfig(wsUrl));
  client.set_on_message(
    ((JSAny? frame) {
      final text = frame.dartify();
      if (text is String) captureInputPointer(text);
    }).toJS,
  );
  client.connect();
  await waitFor(
    'WebSocket connection',
    () => client.get_state() == 'connected' ? true : null,
  );
  final agentId = await waitFor('agent_id', () {
    final id = client.get_agent_id();
    return id.isEmpty ? null : id;
  });
  log('connected as $agentId');

  log('waiting for the math1-input pointer broadcast');
  final pointer = await waitFor('math1-input pointer', () => inputPointer);

  // Empty baseUrl -> requests resolve against the page origin. Every browser module is served from
  // the same ws-server that owns its storage (mirrors the Rust math1 module).
  final rest = RestClient(Dio(BaseOptions(baseUrl: '')));

  log('reading input from /storage/${pointer.bucket}/${pointer.filename}');
  final inputBytes = await rest.storage.getFile(
    agentId: pointer.bucket,
    filename: pointer.filename,
  );
  final input = jsonDecode(utf8.decode(inputBytes)) as Map<String, dynamic>;

  final clientCount = (input['clients'] as List).length;
  log(
    'running FedAvg - $clientCount clients x ${input['rounds']} rounds x ${input['epochs']} local epochs',
  );
  final model = fedAvg(input);
  final weight = model[0];
  final bias = model[1];
  log('global model weight=$weight bias=$bias');

  final output = jsonEncode({
    'module': 'dart-math1',
    'weight': weight,
    'bias': bias,
  });
  await rest.storage.putFile(
    agentId: agentId,
    filename: 'math1-output.json',
    body: Uint8List.fromList(utf8.encode(output)),
  );
  log('stored the global model to /storage/$agentId/math1-output.json');

  await sleep(2000);
  client.disconnect();
  log('workflow complete');
}

@JS('dartMath1Run')
external set _dartMath1Run(JSFunction f);

void main() {
  _dartMath1Run = (() {
    return (() async {
      try {
        await run();
      } catch (e, st) {
        throw '$e\n$st'.toJS;
      }
    }().toJS);
  }.toJS);
}
