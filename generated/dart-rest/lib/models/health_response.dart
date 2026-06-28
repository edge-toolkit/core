// coverage:ignore-file
// GENERATED CODE - DO NOT MODIFY BY HAND
// ignore_for_file: type=lint, unused_import, invalid_annotation_target, unnecessary_import

import 'package:json_annotation/json_annotation.dart';

part 'health_response.g.dart';

/// Server liveness probe response.
///
/// Returned by `GET /health`.
@JsonSerializable()
class HealthResponse {
  const HealthResponse({required this.service, required this.status});

  factory HealthResponse.fromJson(Map<String, Object?> json) =>
      _$HealthResponseFromJson(json);

  final String service;
  final String status;

  Map<String, Object?> toJson() => _$HealthResponseToJson(this);
}
