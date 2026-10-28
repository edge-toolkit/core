// coverage:ignore-file
// GENERATED CODE - DO NOT MODIFY BY HAND
// ignore_for_file: type=lint, unused_import, invalid_annotation_target, unnecessary_import

import 'package:json_annotation/json_annotation.dart';

part 'evicted_agent.g.dart';

/// An agent the hub has evicted, and the address now refused because of it.
@JsonSerializable()
class EvictedAgent {
  const EvictedAgent({required this.agentId, required this.blockedIp});

  factory EvictedAgent.fromJson(Map<String, Object?> json) =>
      _$EvictedAgentFromJson(json);

  @JsonKey(name: 'agent_id')
  final String agentId;
  @JsonKey(name: 'blocked_ip')
  final String blockedIp;

  Map<String, Object?> toJson() => _$EvictedAgentToJson(this);
}
