// coverage:ignore-file
// GENERATED CODE - DO NOT MODIFY BY HAND
// ignore_for_file: type=lint, unused_import, invalid_annotation_target, unnecessary_import

import 'package:dio/dio.dart';
import 'package:retrofit/retrofit.dart';

import '../models/health_response.dart';

part 'health.g.dart';

@RestApi()
abstract class Health {
  factory Health(Dio dio, {String? baseUrl}) = _Health;

  /// Liveness probe.
  ///
  /// Returns a small JSON document identifying the service so external.
  /// monitors can confirm the server is reachable and serving requests.
  @GET('/health')
  Future<HealthResponse> health();
}
