// coverage:ignore-file
// GENERATED CODE - DO NOT MODIFY BY HAND
// ignore_for_file: type=lint, unused_import, invalid_annotation_target, unnecessary_import

import 'package:dio/dio.dart';

import 'clients/health.dart';
import 'clients/modules.dart';
import 'clients/storage.dart';

/// Edge Toolkit REST API `v0.1.0`.
///
/// ws-server HTTP surface: health probe, module discovery, module assets, per-agent storage.
class RestClient {
  RestClient(Dio dio, {String? baseUrl}) : _dio = dio, _baseUrl = baseUrl;

  final Dio _dio;
  final String? _baseUrl;

  static String get version => '0.1.0';

  Health? _health;
  Modules? _modules;
  Storage? _storage;

  Health get health => _health ??= Health(_dio, baseUrl: _baseUrl);

  Modules get modules => _modules ??= Modules(_dio, baseUrl: _baseUrl);

  Storage get storage => _storage ??= Storage(_dio, baseUrl: _baseUrl);
}
