// coverage:ignore-file
// GENERATED CODE - DO NOT MODIFY BY HAND
// ignore_for_file: type=lint, unused_import, invalid_annotation_target, unnecessary_import

import 'package:dio/dio.dart';
import 'package:retrofit/retrofit.dart';

import '../models/evicted_agent.dart';

part 'admin.g.dart';

@RestApi()
abstract class Admin {
  factory Admin(Dio dio, {String? baseUrl}) = _Admin;

  /// Evict an agent: block the address it last connected from, and drop the messages it queued.
  ///
  /// The block is the one [`block_ip`] makes, so it takes every agent on that address with it. The direct messages.
  /// the agent sent that are still waiting for their recipients are dropped, so nothing it queued is delivered after.
  /// it is gone.
  ///
  /// [agentId] - Agent to evict.
  @DELETE('/agents/{agent_id}')
  Future<EvictedAgent> evictAgent({@Path('agent_id') required String agentId});

  /// Block an address: refuse it on every hub of the backplane.
  ///
  /// Blocking an address already blocked succeeds again, so the request can be repeated safely. A path segment that.
  /// is not an IPv4 or IPv6 address matches no route and is answered 404.
  ///
  /// [ip] - IPv4 or IPv6 address to block.
  @PUT('/blocked-ips/{ip}')
  Future<void> blockIp({@Path('ip') required String ip});
}
