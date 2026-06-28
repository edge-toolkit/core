// coverage:ignore-file
// GENERATED CODE - DO NOT MODIFY BY HAND
// ignore_for_file: type=lint, unused_import, invalid_annotation_target, unnecessary_import

import 'dart:typed_data';

import 'package:dio/dio.dart';
import 'package:retrofit/retrofit.dart';

part 'storage.g.dart';

@RestApi()
abstract class Storage {
  factory Storage(Dio dio, {String? baseUrl}) = _Storage;

  /// Download a file previously written to the named agent's storage bucket.
  ///
  /// [agentId] - Agent identifier.
  ///
  /// [filename] - Stored filename.
  @GET('/storage/{agent_id}/{filename}')
  @DioResponseType(ResponseType.bytes)
  Future<List<int>> getFile({
    @Path('agent_id') required String agentId,
    @Path('filename') required String filename,
  });

  /// Upload a file to an agent's storage bucket.
  ///
  /// Only the agent that owns the bucket may write to it (the agent must.
  /// currently be connected); the path component must be a single.
  /// filename, not a nested path.
  ///
  /// [agentId] - Agent identifier (must be a connected agent).
  ///
  /// [filename] - Single-segment filename to write.
  ///
  /// [body] - Phantom type used to label binary request/response bodies as `string`/`binary`.
  ///
  /// Never constructed at runtime; only exists under the `openapi-spec` feature.
  /// so the `utoipa::ToSchema` derive has something to attach to.
  @PUT('/storage/{agent_id}/{filename}')
  Future<void> putFile({
    @Path('agent_id') required String agentId,
    @Path('filename') required String filename,
    @Body() required Uint8List body,
  });
}
