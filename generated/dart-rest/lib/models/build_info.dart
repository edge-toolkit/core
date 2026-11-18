// coverage:ignore-file
// GENERATED CODE - DO NOT MODIFY BY HAND
// ignore_for_file: type=lint, unused_import, invalid_annotation_target, unnecessary_import

import 'package:json_annotation/json_annotation.dart';

part 'build_info.g.dart';

/// How the answering server was built.
///
/// Returned by `GET /info`, so an operator can tell which release, toolchain and feature set a hub is running.
@JsonSerializable()
class BuildInfo {
  const BuildInfo({
    required this.features,
    required this.name,
    required this.profile,
    required this.rustc,
    required this.target,
    required this.version,
  });

  factory BuildInfo.fromJson(Map<String, Object?> json) =>
      _$BuildInfoFromJson(json);

  /// The crate features it was built with, which name the services it can mount.
  final List<String> features;

  /// The crate the server was built from.
  final String name;

  /// The cargo profile, `debug` or `release`.
  final String profile;

  /// The `rustc --version` that compiled it.
  final String rustc;

  /// The target triple it was compiled for.
  final String target;

  /// Its version.
  final String version;

  Map<String, Object?> toJson() => _$BuildInfoToJson(this);
}
