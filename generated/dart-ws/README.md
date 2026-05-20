# et_ws — Dart client for the Edge Toolkit WS protocol

`lib/ws_messages.dart` is regenerated from `edge_toolkit::ws::WsMessage` via
`mise run gen-ws-spec`. `pubspec.yaml` and this README are checked in by hand.

## Usage

Plain Dart 3 sealed classes — no `build_runner`, no extra dependencies. Add
this package as a path dependency in your consumer's `pubspec.yaml`:

```yaml
dependencies:
  et_ws:
    path: ../../../generated/dart-ws
```

Then:

```dart
import 'package:et_ws/ws_messages.dart';

final msg = WsMessage.fromJson(jsonDecode(rawText));
switch (msg) {
  case WsAgentMessage(:final fromAgentId, :final message):
    handle(fromAgentId, message);
  case WsListAgentsResponse(:final agents):
    update(agents);
  default:
    // ignore
}

ws.send(jsonEncode(WsBroadcastMessage(message: payload).toJson()));
```
