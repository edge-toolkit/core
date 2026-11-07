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
  appendOutput('[dart-data1] $msg');
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

Future<void> waitForConnected(WsClient client) async {
  for (var i = 0; i < 100; i++) {
    if (client.get_state() == 'connected') return;
    await sleep(100);
  }
  throw Exception('Timeout waiting for WebSocket connection');
}

Future<String> waitForAgentId(WsClient client) async {
  for (var i = 0; i < 100; i++) {
    final id = client.get_agent_id();
    if (id.isNotEmpty) return id;
    await sleep(100);
  }
  throw Exception('Timeout waiting for agent_id');
}

Future<void> run() async {
  log('entered run()');

  final client = WsClient(WsClientConfig(wsUrl));
  client.connect();
  await waitForConnected(client);
  final agentId = await waitForAgentId(client);
  log('connected as $agentId');

  // Empty baseUrl -> requests resolve against the page origin. Every browser module is served from the same
  // ws-server that owns its storage bucket, so relative paths are what we want (mirrors the Rust data1 module).
  final rest = RestClient(Dio(BaseOptions(baseUrl: '')));

  const filename = 'test_data.txt';
  final content =
      'Hello from dart-data1 at ${DateTime.now().toIso8601String()}!';

  log('storing data to /storage/$agentId/$filename');
  await rest.storage.putFile(
    agentId: agentId,
    filename: filename,
    body: Uint8List.fromList(utf8.encode(content)),
  );

  log('fetching data from /storage/$agentId/$filename');
  final retrieved = await rest.storage.getFile(
    agentId: agentId,
    filename: filename,
  );
  final retrievedContent = utf8.decode(retrieved);

  if (retrievedContent == content) {
    log('VERIFICATION SUCCESS - data matches!');
  } else {
    log(
      'VERIFICATION FAILURE - data mismatch!\nSent: $content\nGot: $retrievedContent',
    );
    throw Exception('Data mismatch');
  }

  await sleep(2000);
  client.disconnect();
  log('workflow complete');
}

@JS('dartData1Run')
external set _dartData1Run(JSFunction f);

void main() {
  _dartData1Run = (() {
    return (() async {
      try {
        await run();
      } catch (e, st) {
        throw '$e\n$st'.toJS;
      }
    }().toJS);
  }.toJS);
}
