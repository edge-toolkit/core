// coverage:ignore-file
// GENERATED CODE - DO NOT MODIFY BY HAND
// ignore_for_file: type=lint, unused_import, invalid_annotation_target, unnecessary_import

import 'package:dio/dio.dart';

import 'clients/admin.dart';
import 'clients/health.dart';
import 'clients/modules.dart';
import 'clients/storage.dart';

/// Edge Toolkit REST API `v0.1.0`.
///
/// ws-server HTTP surface: health probe, module discovery, module assets, per-agent storage, and.
/// the admin routes that block client addresses and evict agents. The storage routes are an anonymous S3-compatible.
/// interface, addressed path-style as /storage/{agent_id}/{filename} (bucket = agent_id, key = filename), they answer.
/// PUT/GET/HEAD with an ETag, so a standard S3 client can read and write objects without credentials. A client address.
/// the hub has blocked is refused by them with 403. The admin routes take the hub's WS_ADMIN_TOKEN as a bearer token.
class RestClient {
  RestClient(Dio dio, {String? baseUrl}) : _dio = dio, _baseUrl = baseUrl;

  final Dio _dio;
  final String? _baseUrl;

  static String get version => '0.1.0';

  Admin? _admin;
  Health? _health;
  Modules? _modules;
  Storage? _storage;

  Admin get admin => _admin ??= Admin(_dio, baseUrl: _baseUrl);

  Health get health => _health ??= Health(_dio, baseUrl: _baseUrl);

  Modules get modules => _modules ??= Modules(_dio, baseUrl: _baseUrl);

  Storage get storage => _storage ??= Storage(_dio, baseUrl: _baseUrl);
}
