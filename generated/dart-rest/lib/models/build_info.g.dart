// GENERATED CODE - DO NOT MODIFY BY HAND

part of 'build_info.dart';

// **************************************************************************
// JsonSerializableGenerator
// **************************************************************************

BuildInfo _$BuildInfoFromJson(Map<String, dynamic> json) => BuildInfo(
  features: (json['features'] as List<dynamic>)
      .map((e) => e as String)
      .toList(),
  name: json['name'] as String,
  profile: json['profile'] as String,
  rustc: json['rustc'] as String,
  target: json['target'] as String,
  version: json['version'] as String,
);

Map<String, dynamic> _$BuildInfoToJson(BuildInfo instance) => <String, dynamic>{
  'features': instance.features,
  'name': instance.name,
  'profile': instance.profile,
  'rustc': instance.rustc,
  'target': instance.target,
  'version': instance.version,
};
