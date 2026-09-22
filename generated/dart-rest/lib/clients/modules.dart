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
  /// `path` is resolved relative to the module's bundle root; an unknown module or missing file returns 404.
  ///
  /// Both path parameters can themselves contain `/`. A module is served under the name its `package.json`.
  /// declares, which carries an owner scope (`@scope/name`) for anything published to a registry, and `path`.
  /// addresses sub-directories of the bundle. A client that percent-encodes each parameter as one path segment.
  /// turns those slashes into `%2F` and asks for something no server serves, so build the request path rather.
  /// than passing the values through a per-segment encoder.
  ///
  /// [name] - Module name, as its package.json declares it -- may be scoped.
  ///
  /// [path] - Path of the file within the module bundle.
  @GET('/modules/{name}/{path}')
  Future<void> getModuleFile({
    @Path('name') required String name,
    @Path('path') required String path,
  });
}
