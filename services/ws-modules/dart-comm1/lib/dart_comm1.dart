import 'dart:async';
import 'dart:js_interop';

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
  external String get_client_id();
  external void send(String message);
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
  appendOutput('[dart-comm1] $msg');
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
    final id = client.get_client_id();
    if (id.isNotEmpty) return id;
    await sleep(100);
  }
  throw Exception('Timeout waiting for agent_id');
}

Future<void> run() async {
  log('entered run()');

  final client = WsClient(WsClientConfig(wsUrl));

  String selfAgentId = '';
  String? targetAgentId;

  client.set_on_message(
    ((JSString raw) {
      final data = raw.toDart;
      try {
        // Parse type field manually to avoid a JSON dep
        if (data.contains('"list_agents_response"')) {
          // Extract first other connected agent id
          final idMatches = RegExp(
            r'"agent_id"\s*:\s*"([^"]+)"',
          ).allMatches(data);
          for (final m in idMatches) {
            final id = m.group(1)!;
            if (id != selfAgentId) {
              targetAgentId = id;
              break;
            }
          }
        } else if (data.contains('"agent_message"') ||
            data.contains('"message_status"')) {
          log('received: $data');
          appendOutput(data);
        }
      } catch (_) {}
    }).toJS,
  );

  client.connect();
  await waitForConnected(client);
  selfAgentId = await waitForAgentId(client);
  log('connected as $selfAgentId');

  // Poll for a peer agent
  while (targetAgentId == null) {
    client.send('{"type":"list_agents"}');
    await sleep(1000);
  }

  log('found peer $targetAgentId, sending broadcast');
  client.send(
    '{"type":"broadcast_message","message":{"module":"dart-comm1","step":"broadcast","from_agent_id":"$selfAgentId","message":"dart-comm1 broadcast to all other connected agents"}}',
  );

  await sleep(3000);

  log('sending direct message to $targetAgentId');
  client.send(
    '{"type":"send_agent_message","to_agent_id":"$targetAgentId","message":{"module":"dart-comm1","step":"direct","from_agent_id":"$selfAgentId","message":"dart-comm1 direct message"}}',
  );

  await sleep(3000);
  client.disconnect();
  log('workflow complete');
}

@JS('dartComm1Run')
external set _dartComm1Run(JSFunction f);

void main() {
  _dartComm1Run = (() {
    return (() async {
      try {
        await run();
      } catch (e, st) {
        throw '$e\n$st'.toJS;
      }
    }().toJS);
  }.toJS);
}
