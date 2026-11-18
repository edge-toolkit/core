// coverage:ignore-file
// GENERATED CODE - DO NOT MODIFY BY HAND
// ignore_for_file: type=lint, unused_import, invalid_annotation_target, unnecessary_import

import 'package:dio/dio.dart';
import 'package:retrofit/retrofit.dart';

import '../models/build_info.dart';

part 'info.g.dart';

@RestApi()
abstract class Info {
  factory Info(Dio dio, {String? baseUrl}) = _Info;

  /// Build information.
  ///
  /// Returns the release, toolchain, target and features the server was built with.
  @GET('/info')
  Future<BuildInfo> info();
}
