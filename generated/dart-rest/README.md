# et_rest -- Dart client for the Edge Toolkit REST API

`lib/` is regenerated from `generated/specs/rest.yaml` (OpenAPI 3.0) by `mise run gen:dart-rest`, which runs
`swagger_parser` then `build_runner`. `pubspec.yaml`, `swagger_parser.yaml`, and this README are checked in by hand.

The retrofit clients are dio-backed. Under `dart compile js` dio selects its browser adapter automatically, so the
same client runs in a ws-module compiled to JavaScript with no `dart:io` dependency.

## Storage endpoints carry raw bytes

`swagger_parser` maps the `application/octet-stream` storage endpoints to a `dart:io` `File` body and a `void`
response, neither of which is usable in the browser. `gen:dart-rest` post-processes `clients/storage.dart` so
`putFile` takes a `Uint8List` body (dio sends `Uint8List` as raw bytes -- a plain `List<int>` would be JSON-encoded by
dio's request transformer) and `getFile` returns `List<int>` via `@DioResponseType(ResponseType.bytes)`.

The `client_postfix: ""` setting in `swagger_parser.yaml` drops the `_client` suffix (`clients/storage.dart`, class
`Storage`); `gen:dart-rest` also fixes up `rest_client.dart`, whose sub-client imports swagger_parser emits with a
stray trailing underscore under that setting.

## Usage

Add the package as a path dependency in your consumer's `pubspec.yaml`:

```yaml
dependencies:
  et_rest:
    path: ../../../generated/dart-rest
```

Then:

```dart
import 'package:dio/dio.dart';
import 'package:et_rest/export.dart';

// Empty baseUrl -> requests resolve against the page origin (the ws-server that owns the storage bucket).
final rest = RestClient(Dio(BaseOptions(baseUrl: '')));
await rest.storage.putFile(agentId: id, filename: 'f.txt', body: bytes);
final got = await rest.storage.getFile(agentId: id, filename: 'f.txt');
```
