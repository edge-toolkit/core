// coverage:ignore-file
// GENERATED CODE - DO NOT MODIFY BY HAND
// ignore_for_file: type=lint, unused_import, invalid_annotation_target, unnecessary_import

import 'package:dio/dio.dart';
import 'package:retrofit/retrofit.dart';

part 'modules.g.dart';

@RestApi()
abstract class Modules {
  factory Modules(Dio dio, {String? baseUrl}) = _Modules;

  /// List the names of every module the server is currently serving.
  @GET('/modules/')
  Future<List<String>> listModulesHandler();

  /// Fetch a file from a module's bundled static assets.
  ///
  /// `path` is resolved relative to the module's bundle root; an unknown.
  /// module or missing file returns 404.
  ///
  /// [name] - Module name.
  ///
  /// [path] - Path of the file within the module bundle.
  @GET('/modules/{name}/{path}')
  Future<void> getModuleFile({
    @Path('name') required String name,
    @Path('path') required String path,
  });
}
