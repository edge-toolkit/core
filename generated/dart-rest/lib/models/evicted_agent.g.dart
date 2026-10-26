// GENERATED CODE - DO NOT MODIFY BY HAND

part of 'evicted_agent.dart';

// **************************************************************************
// JsonSerializableGenerator
// **************************************************************************

EvictedAgent _$EvictedAgentFromJson(Map<String, dynamic> json) => EvictedAgent(
  agentId: json['agent_id'] as String,
  blockedIp: json['blocked_ip'] as String,
);

Map<String, dynamic> _$EvictedAgentToJson(EvictedAgent instance) =>
    <String, dynamic>{
      'agent_id': instance.agentId,
      'blocked_ip': instance.blockedIp,
    };
