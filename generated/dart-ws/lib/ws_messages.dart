// ignore_for_file: unnecessary_cast
final class AgentSummary {
  final String agentId;
  final String? lastKnownIp;
  final AgentConnectionState state;

  const AgentSummary({
    required this.agentId,
    this.lastKnownIp,
    required this.state,
  });

  static AgentSummaryBuilder builder({
    required String agentId,
    String? lastKnownIp,
    required AgentConnectionState state,
  }) => AgentSummaryBuilder(
    agentId: agentId,
    lastKnownIp: lastKnownIp == null ? null : (lastKnownIp as String),
    state: state,
  );
  AgentSummaryBuilder toBuilder() => AgentSummaryBuilder(
    agentId: agentId,
    lastKnownIp: lastKnownIp == null ? null : (lastKnownIp as String),
    state: state,
  );

  Map<String, dynamic> toJson() => {
    "agent_id": agentId,
    "last_known_ip": lastKnownIp,
    "state": state.toJson(),
  };
  factory AgentSummary.fromJson(Map<String, dynamic> json) => AgentSummary(
    agentId: json["agent_id"] as String,
    lastKnownIp: json["last_known_ip"] == null
        ? null
        : json["last_known_ip"] == null
        ? null
        : json["last_known_ip"] as String,
    state: AgentConnectionState.fromJson(json["state"]),
  );

  @override
  String toString() =>
      "AgentSummary("
      "agentId: $agentId, "
      "lastKnownIp: $lastKnownIp, "
      "state: $state"
      ")";
  @override
  bool operator ==(Object other) {
    if (identical(this, other)) {
      return true;
    }
    if (other is! AgentSummary) {
      return false;
    }
    if (agentId != other.agentId) {
      return false;
    }
    if (lastKnownIp != other.lastKnownIp) {
      return false;
    }
    if (state != other.state) {
      return false;
    }
    return true;
  }

  @override
  int get hashCode =>
      Object.hashAll([agentId.hashCode, lastKnownIp?.hashCode, state.hashCode]);
}

/// Builder class for [AgentSummary]
final class AgentSummaryBuilder {
  String agentId;
  String? lastKnownIp;
  AgentConnectionState state;

  AgentSummaryBuilder({
    required this.agentId,
    required this.lastKnownIp,
    required this.state,
  });

  AgentSummary build() => AgentSummary(
    agentId: agentId,
    lastKnownIp: lastKnownIp == null ? null : (lastKnownIp as String),
    state: state,
  );
}

sealed class WsMessage {
  const WsMessage();

  WsMessageBuilder toBuilder();

  Map<String, dynamic> toJson();
  factory WsMessage.fromJson(Map<String, dynamic> json) =>
      switch (json["type"]) {
        "et-connect" => WsConnect.fromJson(json),
        "et-connect-ack" => WsConnectAck.fromJson(json),
        "et-alive" => WsAlive.fromJson(json),
        "et-list-agents" => WsListAgents.fromJson(json),
        "et-list-agents-response" => WsListAgentsResponse.fromJson(json),
        "et-send-agent-message" => WsSendAgentMessage.fromJson(json),
        "et-broadcast-message" => WsBroadcastMessage.fromJson(json),
        "et-agent-message" => WsAgentMessage.fromJson(json),
        "et-message-ack" => WsMessageAck.fromJson(json),
        "et-message-status" => WsMessageStatus.fromJson(json),
        "et-invalid" => WsInvalid.fromJson(json),
        "et-client-event" => WsClientEvent.fromJson(json),
        "et-response" => WsResponse.fromJson(json),
        final other => throw ArgumentError("unknown discriminant: $other"),
      };
}

sealed class WsMessageBuilder {
  WsMessage build();
}

final class WsConnect extends WsMessage {
  final String? agentId;

  const WsConnect({this.agentId}) : super();

  static WsConnectBuilder builder({String? agentId}) =>
      WsConnectBuilder(agentId: agentId == null ? null : (agentId as String));
  WsConnectBuilder toBuilder() =>
      WsConnectBuilder(agentId: agentId == null ? null : (agentId as String));

  @override
  Map<String, dynamic> toJson() => {"agent_id": agentId, "type": "et-connect"};
  factory WsConnect.fromJson(Map<String, dynamic> json) => WsConnect(
    agentId: json["agent_id"] == null
        ? null
        : json["agent_id"] == null
        ? null
        : json["agent_id"] as String,
  );

  @override
  String toString() =>
      "WsConnect("
      "agentId: $agentId"
      ")";
  @override
  bool operator ==(Object other) {
    if (identical(this, other)) {
      return true;
    }
    if (other is! WsConnect) {
      return false;
    }
    if (agentId != other.agentId) {
      return false;
    }
    return true;
  }

  @override
  int get hashCode => Object.hashAll([agentId?.hashCode]);
}

/// Builder class for [WsConnect]
final class WsConnectBuilder extends WsMessageBuilder {
  String? agentId;

  WsConnectBuilder({required this.agentId}) : super();

  WsConnect build() =>
      WsConnect(agentId: agentId == null ? null : (agentId as String));
}

final class WsConnectAck extends WsMessage {
  final String agentId;
  final ConnectStatus status;

  const WsConnectAck({required this.agentId, required this.status}) : super();

  static WsConnectAckBuilder builder({
    required String agentId,
    required ConnectStatus status,
  }) => WsConnectAckBuilder(agentId: agentId, status: status);
  WsConnectAckBuilder toBuilder() =>
      WsConnectAckBuilder(agentId: agentId, status: status);

  @override
  Map<String, dynamic> toJson() => {
    "agent_id": agentId,
    "status": status.toJson(),
    "type": "et-connect-ack",
  };
  factory WsConnectAck.fromJson(Map<String, dynamic> json) => WsConnectAck(
    agentId: json["agent_id"] as String,
    status: ConnectStatus.fromJson(json["status"]),
  );

  @override
  String toString() =>
      "WsConnectAck("
      "agentId: $agentId, "
      "status: $status"
      ")";
  @override
  bool operator ==(Object other) {
    if (identical(this, other)) {
      return true;
    }
    if (other is! WsConnectAck) {
      return false;
    }
    if (agentId != other.agentId) {
      return false;
    }
    if (status != other.status) {
      return false;
    }
    return true;
  }

  @override
  int get hashCode => Object.hashAll([agentId.hashCode, status.hashCode]);
}

/// Builder class for [WsConnectAck]
final class WsConnectAckBuilder extends WsMessageBuilder {
  String agentId;
  ConnectStatus status;

  WsConnectAckBuilder({required this.agentId, required this.status}) : super();

  WsConnectAck build() => WsConnectAck(agentId: agentId, status: status);
}

final class WsAlive extends WsMessage {
  final String timestamp;

  const WsAlive({required this.timestamp}) : super();

  static WsAliveBuilder builder({required String timestamp}) =>
      WsAliveBuilder(timestamp: timestamp);
  WsAliveBuilder toBuilder() => WsAliveBuilder(timestamp: timestamp);

  @override
  Map<String, dynamic> toJson() => {"timestamp": timestamp, "type": "et-alive"};
  factory WsAlive.fromJson(Map<String, dynamic> json) =>
      WsAlive(timestamp: json["timestamp"] as String);

  @override
  String toString() =>
      "WsAlive("
      "timestamp: $timestamp"
      ")";
  @override
  bool operator ==(Object other) {
    if (identical(this, other)) {
      return true;
    }
    if (other is! WsAlive) {
      return false;
    }
    if (timestamp != other.timestamp) {
      return false;
    }
    return true;
  }

  @override
  int get hashCode => Object.hashAll([timestamp.hashCode]);
}

/// Builder class for [WsAlive]
final class WsAliveBuilder extends WsMessageBuilder {
  String timestamp;

  WsAliveBuilder({required this.timestamp}) : super();

  WsAlive build() => WsAlive(timestamp: timestamp);
}

final class WsListAgents extends WsMessage {
  const WsListAgents() : super();

  static WsListAgentsBuilder builder() => WsListAgentsBuilder();
  WsListAgentsBuilder toBuilder() => WsListAgentsBuilder();

  @override
  Map<String, dynamic> toJson() => {"type": "et-list-agents"};
  factory WsListAgents.fromJson(Map<String, dynamic> json) => WsListAgents();

  @override
  String toString() =>
      "WsListAgents("
      ")";
  @override
  bool operator ==(Object other) {
    if (identical(this, other)) {
      return true;
    }
    if (other is! WsListAgents) {
      return false;
    }
    return true;
  }

  @override
  int get hashCode => Object.hashAll([]);
}

/// Builder class for [WsListAgents]
final class WsListAgentsBuilder extends WsMessageBuilder {
  WsListAgentsBuilder() : super();

  WsListAgents build() => WsListAgents();
}

final class WsListAgentsResponse extends WsMessage {
  final List<AgentSummary> agents;

  const WsListAgentsResponse({required this.agents}) : super();

  static WsListAgentsResponseBuilder builder({
    required List<AgentSummary> agents,
  }) => WsListAgentsResponseBuilder(
    agents: agents.map((elem) => elem.toBuilder()).toList(),
  );
  WsListAgentsResponseBuilder toBuilder() => WsListAgentsResponseBuilder(
    agents: agents.map((elem) => elem.toBuilder()).toList(),
  );

  @override
  Map<String, dynamic> toJson() => {
    "agents": agents.map((inner) => inner.toJson()).toList(),
    "type": "et-list-agents-response",
  };
  factory WsListAgentsResponse.fromJson(Map<String, dynamic> json) =>
      WsListAgentsResponse(
        agents: (json["agents"] as List<dynamic>)
            .map<AgentSummary>(
              (inner) => AgentSummary.fromJson(inner as Map<String, dynamic>),
            )
            .toList(),
      );

  @override
  String toString() =>
      "WsListAgentsResponse("
      "agents: $agents"
      ")";
  @override
  bool operator ==(Object other) {
    if (identical(this, other)) {
      return true;
    }
    if (other is! WsListAgentsResponse) {
      return false;
    }
    if (agents.length != other.agents.length) {
      return false;
    }
    for (var i = 0; i < agents.length; i++) {
      if (agents[i] != other.agents[i]) {
        return false;
      }
    }
    return true;
  }

  @override
  int get hashCode =>
      Object.hashAll([Object.hashAll(agents.map((elem) => elem.hashCode))]);
}

/// Builder class for [WsListAgentsResponse]
final class WsListAgentsResponseBuilder extends WsMessageBuilder {
  List<AgentSummaryBuilder> agents;

  WsListAgentsResponseBuilder({required this.agents}) : super();

  WsListAgentsResponse build() =>
      WsListAgentsResponse(agents: agents.map((elem) => elem.build()).toList());
}

final class WsSendAgentMessage extends WsMessage {
  final Map<String, dynamic> message;
  final String toAgentId;

  const WsSendAgentMessage({required this.message, required this.toAgentId})
    : super();

  static WsSendAgentMessageBuilder builder({
    required Map<String, dynamic> message,
    required String toAgentId,
  }) => WsSendAgentMessageBuilder(
    message: message.map((key, value) => MapEntry(key, value)),
    toAgentId: toAgentId,
  );
  WsSendAgentMessageBuilder toBuilder() => WsSendAgentMessageBuilder(
    message: message.map((key, value) => MapEntry(key, value)),
    toAgentId: toAgentId,
  );

  @override
  Map<String, dynamic> toJson() => {
    "message": message.map((key, value) => MapEntry(key, value)),
    "to_agent_id": toAgentId,
    "type": "et-send-agent-message",
  };
  factory WsSendAgentMessage.fromJson(Map<String, dynamic> json) =>
      WsSendAgentMessage(
        message: (json["message"] as Map).map<String, dynamic>(
          (key, value) => MapEntry(key as String, value as dynamic),
        ),
        toAgentId: json["to_agent_id"] as String,
      );

  @override
  String toString() =>
      "WsSendAgentMessage("
      "message: $message, "
      "toAgentId: $toAgentId"
      ")";
  @override
  bool operator ==(Object other) {
    if (identical(this, other)) {
      return true;
    }
    if (other is! WsSendAgentMessage) {
      return false;
    }
    if (message.length != other.message.length) {
      return false;
    }
    for (final entry in message.entries) {
      if (entry.value != other.message[entry.key]) {
        return false;
      }
    }
    if (toAgentId != other.toAgentId) {
      return false;
    }
    return true;
  }

  @override
  int get hashCode => Object.hashAll([
    Object.hashAll(
      message.entries.expand((entry) => [entry.key, entry.value.hashCode]),
    ),
    toAgentId.hashCode,
  ]);
}

/// Builder class for [WsSendAgentMessage]
final class WsSendAgentMessageBuilder extends WsMessageBuilder {
  Map<String, dynamic> message;
  String toAgentId;

  WsSendAgentMessageBuilder({required this.message, required this.toAgentId})
    : super();

  WsSendAgentMessage build() => WsSendAgentMessage(
    message: message.map((key, value) => MapEntry(key, value)),
    toAgentId: toAgentId,
  );
}

final class WsBroadcastMessage extends WsMessage {
  final Map<String, dynamic> message;

  const WsBroadcastMessage({required this.message}) : super();

  static WsBroadcastMessageBuilder builder({
    required Map<String, dynamic> message,
  }) => WsBroadcastMessageBuilder(
    message: message.map((key, value) => MapEntry(key, value)),
  );
  WsBroadcastMessageBuilder toBuilder() => WsBroadcastMessageBuilder(
    message: message.map((key, value) => MapEntry(key, value)),
  );

  @override
  Map<String, dynamic> toJson() => {
    "message": message.map((key, value) => MapEntry(key, value)),
    "type": "et-broadcast-message",
  };
  factory WsBroadcastMessage.fromJson(Map<String, dynamic> json) =>
      WsBroadcastMessage(
        message: (json["message"] as Map).map<String, dynamic>(
          (key, value) => MapEntry(key as String, value as dynamic),
        ),
      );

  @override
  String toString() =>
      "WsBroadcastMessage("
      "message: $message"
      ")";
  @override
  bool operator ==(Object other) {
    if (identical(this, other)) {
      return true;
    }
    if (other is! WsBroadcastMessage) {
      return false;
    }
    if (message.length != other.message.length) {
      return false;
    }
    for (final entry in message.entries) {
      if (entry.value != other.message[entry.key]) {
        return false;
      }
    }
    return true;
  }

  @override
  int get hashCode => Object.hashAll([
    Object.hashAll(
      message.entries.expand((entry) => [entry.key, entry.value.hashCode]),
    ),
  ]);
}

/// Builder class for [WsBroadcastMessage]
final class WsBroadcastMessageBuilder extends WsMessageBuilder {
  Map<String, dynamic> message;

  WsBroadcastMessageBuilder({required this.message}) : super();

  WsBroadcastMessage build() => WsBroadcastMessage(
    message: message.map((key, value) => MapEntry(key, value)),
  );
}

final class WsAgentMessage extends WsMessage {
  final String fromAgentId;
  final Map<String, dynamic> message;
  final String messageId;
  final MessageScope scope;
  final String serverReceivedAt;

  const WsAgentMessage({
    required this.fromAgentId,
    required this.message,
    required this.messageId,
    required this.scope,
    required this.serverReceivedAt,
  }) : super();

  static WsAgentMessageBuilder builder({
    required String fromAgentId,
    required Map<String, dynamic> message,
    required String messageId,
    required MessageScope scope,
    required String serverReceivedAt,
  }) => WsAgentMessageBuilder(
    fromAgentId: fromAgentId,
    message: message.map((key, value) => MapEntry(key, value)),
    messageId: messageId,
    scope: scope,
    serverReceivedAt: serverReceivedAt,
  );
  WsAgentMessageBuilder toBuilder() => WsAgentMessageBuilder(
    fromAgentId: fromAgentId,
    message: message.map((key, value) => MapEntry(key, value)),
    messageId: messageId,
    scope: scope,
    serverReceivedAt: serverReceivedAt,
  );

  @override
  Map<String, dynamic> toJson() => {
    "from_agent_id": fromAgentId,
    "message": message.map((key, value) => MapEntry(key, value)),
    "message_id": messageId,
    "scope": scope.toJson(),
    "server_received_at": serverReceivedAt,
    "type": "et-agent-message",
  };
  factory WsAgentMessage.fromJson(Map<String, dynamic> json) => WsAgentMessage(
    fromAgentId: json["from_agent_id"] as String,
    message: (json["message"] as Map).map<String, dynamic>(
      (key, value) => MapEntry(key as String, value as dynamic),
    ),
    messageId: json["message_id"] as String,
    scope: MessageScope.fromJson(json["scope"]),
    serverReceivedAt: json["server_received_at"] as String,
  );

  @override
  String toString() =>
      "WsAgentMessage("
      "fromAgentId: $fromAgentId, "
      "message: $message, "
      "messageId: $messageId, "
      "scope: $scope, "
      "serverReceivedAt: $serverReceivedAt"
      ")";
  @override
  bool operator ==(Object other) {
    if (identical(this, other)) {
      return true;
    }
    if (other is! WsAgentMessage) {
      return false;
    }
    if (fromAgentId != other.fromAgentId) {
      return false;
    }
    if (message.length != other.message.length) {
      return false;
    }
    for (final entry in message.entries) {
      if (entry.value != other.message[entry.key]) {
        return false;
      }
    }
    if (messageId != other.messageId) {
      return false;
    }
    if (scope != other.scope) {
      return false;
    }
    if (serverReceivedAt != other.serverReceivedAt) {
      return false;
    }
    return true;
  }

  @override
  int get hashCode => Object.hashAll([
    fromAgentId.hashCode,
    Object.hashAll(
      message.entries.expand((entry) => [entry.key, entry.value.hashCode]),
    ),
    messageId.hashCode,
    scope.hashCode,
    serverReceivedAt.hashCode,
  ]);
}

/// Builder class for [WsAgentMessage]
final class WsAgentMessageBuilder extends WsMessageBuilder {
  String fromAgentId;
  Map<String, dynamic> message;
  String messageId;
  MessageScope scope;
  String serverReceivedAt;

  WsAgentMessageBuilder({
    required this.fromAgentId,
    required this.message,
    required this.messageId,
    required this.scope,
    required this.serverReceivedAt,
  }) : super();

  WsAgentMessage build() => WsAgentMessage(
    fromAgentId: fromAgentId,
    message: message.map((key, value) => MapEntry(key, value)),
    messageId: messageId,
    scope: scope,
    serverReceivedAt: serverReceivedAt,
  );
}

final class WsMessageAck extends WsMessage {
  final String messageId;

  const WsMessageAck({required this.messageId}) : super();

  static WsMessageAckBuilder builder({required String messageId}) =>
      WsMessageAckBuilder(messageId: messageId);
  WsMessageAckBuilder toBuilder() => WsMessageAckBuilder(messageId: messageId);

  @override
  Map<String, dynamic> toJson() => {
    "message_id": messageId,
    "type": "et-message-ack",
  };
  factory WsMessageAck.fromJson(Map<String, dynamic> json) =>
      WsMessageAck(messageId: json["message_id"] as String);

  @override
  String toString() =>
      "WsMessageAck("
      "messageId: $messageId"
      ")";
  @override
  bool operator ==(Object other) {
    if (identical(this, other)) {
      return true;
    }
    if (other is! WsMessageAck) {
      return false;
    }
    if (messageId != other.messageId) {
      return false;
    }
    return true;
  }

  @override
  int get hashCode => Object.hashAll([messageId.hashCode]);
}

/// Builder class for [WsMessageAck]
final class WsMessageAckBuilder extends WsMessageBuilder {
  String messageId;

  WsMessageAckBuilder({required this.messageId}) : super();

  WsMessageAck build() => WsMessageAck(messageId: messageId);
}

final class WsMessageStatus extends WsMessage {
  final String detail;
  final String? messageId;
  final MessageDeliveryStatus status;

  const WsMessageStatus({
    required this.detail,
    this.messageId,
    required this.status,
  }) : super();

  static WsMessageStatusBuilder builder({
    required String detail,
    String? messageId,
    required MessageDeliveryStatus status,
  }) => WsMessageStatusBuilder(
    detail: detail,
    messageId: messageId == null ? null : (messageId as String),
    status: status,
  );
  WsMessageStatusBuilder toBuilder() => WsMessageStatusBuilder(
    detail: detail,
    messageId: messageId == null ? null : (messageId as String),
    status: status,
  );

  @override
  Map<String, dynamic> toJson() => {
    "detail": detail,
    "message_id": messageId,
    "status": status.toJson(),
    "type": "et-message-status",
  };
  factory WsMessageStatus.fromJson(Map<String, dynamic> json) =>
      WsMessageStatus(
        detail: json["detail"] as String,
        messageId: json["message_id"] == null
            ? null
            : json["message_id"] == null
            ? null
            : json["message_id"] as String,
        status: MessageDeliveryStatus.fromJson(json["status"]),
      );

  @override
  String toString() =>
      "WsMessageStatus("
      "detail: $detail, "
      "messageId: $messageId, "
      "status: $status"
      ")";
  @override
  bool operator ==(Object other) {
    if (identical(this, other)) {
      return true;
    }
    if (other is! WsMessageStatus) {
      return false;
    }
    if (detail != other.detail) {
      return false;
    }
    if (messageId != other.messageId) {
      return false;
    }
    if (status != other.status) {
      return false;
    }
    return true;
  }

  @override
  int get hashCode =>
      Object.hashAll([detail.hashCode, messageId?.hashCode, status.hashCode]);
}

/// Builder class for [WsMessageStatus]
final class WsMessageStatusBuilder extends WsMessageBuilder {
  String detail;
  String? messageId;
  MessageDeliveryStatus status;

  WsMessageStatusBuilder({
    required this.detail,
    required this.messageId,
    required this.status,
  }) : super();

  WsMessageStatus build() => WsMessageStatus(
    detail: detail,
    messageId: messageId == null ? null : (messageId as String),
    status: status,
  );
}

final class WsInvalid extends WsMessage {
  final String detail;
  final String? messageId;

  const WsInvalid({required this.detail, this.messageId}) : super();

  static WsInvalidBuilder builder({
    required String detail,
    String? messageId,
  }) => WsInvalidBuilder(
    detail: detail,
    messageId: messageId == null ? null : (messageId as String),
  );
  WsInvalidBuilder toBuilder() => WsInvalidBuilder(
    detail: detail,
    messageId: messageId == null ? null : (messageId as String),
  );

  @override
  Map<String, dynamic> toJson() => {
    "detail": detail,
    "message_id": messageId,
    "type": "et-invalid",
  };
  factory WsInvalid.fromJson(Map<String, dynamic> json) => WsInvalid(
    detail: json["detail"] as String,
    messageId: json["message_id"] == null
        ? null
        : json["message_id"] == null
        ? null
        : json["message_id"] as String,
  );

  @override
  String toString() =>
      "WsInvalid("
      "detail: $detail, "
      "messageId: $messageId"
      ")";
  @override
  bool operator ==(Object other) {
    if (identical(this, other)) {
      return true;
    }
    if (other is! WsInvalid) {
      return false;
    }
    if (detail != other.detail) {
      return false;
    }
    if (messageId != other.messageId) {
      return false;
    }
    return true;
  }

  @override
  int get hashCode => Object.hashAll([detail.hashCode, messageId?.hashCode]);
}

/// Builder class for [WsInvalid]
final class WsInvalidBuilder extends WsMessageBuilder {
  String detail;
  String? messageId;

  WsInvalidBuilder({required this.detail, required this.messageId}) : super();

  WsInvalid build() => WsInvalid(
    detail: detail,
    messageId: messageId == null ? null : (messageId as String),
  );
}

final class WsClientEvent extends WsMessage {
  final String action;
  final String capability;
  final Map<String, dynamic> details;

  const WsClientEvent({
    required this.action,
    required this.capability,
    required this.details,
  }) : super();

  static WsClientEventBuilder builder({
    required String action,
    required String capability,
    required Map<String, dynamic> details,
  }) => WsClientEventBuilder(
    action: action,
    capability: capability,
    details: details.map((key, value) => MapEntry(key, value)),
  );
  WsClientEventBuilder toBuilder() => WsClientEventBuilder(
    action: action,
    capability: capability,
    details: details.map((key, value) => MapEntry(key, value)),
  );

  @override
  Map<String, dynamic> toJson() => {
    "action": action,
    "capability": capability,
    "details": details.map((key, value) => MapEntry(key, value)),
    "type": "et-client-event",
  };
  factory WsClientEvent.fromJson(Map<String, dynamic> json) => WsClientEvent(
    action: json["action"] as String,
    capability: json["capability"] as String,
    details: (json["details"] as Map).map<String, dynamic>(
      (key, value) => MapEntry(key as String, value as dynamic),
    ),
  );

  @override
  String toString() =>
      "WsClientEvent("
      "action: $action, "
      "capability: $capability, "
      "details: $details"
      ")";
  @override
  bool operator ==(Object other) {
    if (identical(this, other)) {
      return true;
    }
    if (other is! WsClientEvent) {
      return false;
    }
    if (action != other.action) {
      return false;
    }
    if (capability != other.capability) {
      return false;
    }
    if (details.length != other.details.length) {
      return false;
    }
    for (final entry in details.entries) {
      if (entry.value != other.details[entry.key]) {
        return false;
      }
    }
    return true;
  }

  @override
  int get hashCode => Object.hashAll([
    action.hashCode,
    capability.hashCode,
    Object.hashAll(
      details.entries.expand((entry) => [entry.key, entry.value.hashCode]),
    ),
  ]);
}

/// Builder class for [WsClientEvent]
final class WsClientEventBuilder extends WsMessageBuilder {
  String action;
  String capability;
  Map<String, dynamic> details;

  WsClientEventBuilder({
    required this.action,
    required this.capability,
    required this.details,
  }) : super();

  WsClientEvent build() => WsClientEvent(
    action: action,
    capability: capability,
    details: details.map((key, value) => MapEntry(key, value)),
  );
}

final class WsResponse extends WsMessage {
  final String message;

  const WsResponse({required this.message}) : super();

  static WsResponseBuilder builder({required String message}) =>
      WsResponseBuilder(message: message);
  WsResponseBuilder toBuilder() => WsResponseBuilder(message: message);

  @override
  Map<String, dynamic> toJson() => {"message": message, "type": "et-response"};
  factory WsResponse.fromJson(Map<String, dynamic> json) =>
      WsResponse(message: json["message"] as String);

  @override
  String toString() =>
      "WsResponse("
      "message: $message"
      ")";
  @override
  bool operator ==(Object other) {
    if (identical(this, other)) {
      return true;
    }
    if (other is! WsResponse) {
      return false;
    }
    if (message != other.message) {
      return false;
    }
    return true;
  }

  @override
  int get hashCode => Object.hashAll([message.hashCode]);
}

/// Builder class for [WsResponse]
final class WsResponseBuilder extends WsMessageBuilder {
  String message;

  WsResponseBuilder({required this.message}) : super();

  WsResponse build() => WsResponse(message: message);
}

enum AgentConnectionState {
  connected,
  disconnected;

  factory AgentConnectionState.fromJson(dynamic json) => switch (json) {
    "connected" => AgentConnectionState.connected,
    "disconnected" => AgentConnectionState.disconnected,
    final other => throw ArgumentError("Unknown variant: $other"),
  };

  dynamic toJson() => switch (this) {
    AgentConnectionState.connected => "connected",
    AgentConnectionState.disconnected => "disconnected",
  };
  @override
  String toString() => switch (this) {
    AgentConnectionState.connected => "connected",
    AgentConnectionState.disconnected => "disconnected",
  };
}

enum ConnectStatus {
  assigned,
  reconnected;

  factory ConnectStatus.fromJson(dynamic json) => switch (json) {
    "assigned" => ConnectStatus.assigned,
    "reconnected" => ConnectStatus.reconnected,
    final other => throw ArgumentError("Unknown variant: $other"),
  };

  dynamic toJson() => switch (this) {
    ConnectStatus.assigned => "assigned",
    ConnectStatus.reconnected => "reconnected",
  };
  @override
  String toString() => switch (this) {
    ConnectStatus.assigned => "assigned",
    ConnectStatus.reconnected => "reconnected",
  };
}

enum MessageDeliveryStatus {
  delivered,
  queued,
  acknowledged,
  broadcast;

  factory MessageDeliveryStatus.fromJson(dynamic json) => switch (json) {
    "delivered" => MessageDeliveryStatus.delivered,
    "queued" => MessageDeliveryStatus.queued,
    "acknowledged" => MessageDeliveryStatus.acknowledged,
    "broadcast" => MessageDeliveryStatus.broadcast,
    final other => throw ArgumentError("Unknown variant: $other"),
  };

  dynamic toJson() => switch (this) {
    MessageDeliveryStatus.delivered => "delivered",
    MessageDeliveryStatus.queued => "queued",
    MessageDeliveryStatus.acknowledged => "acknowledged",
    MessageDeliveryStatus.broadcast => "broadcast",
  };
  @override
  String toString() => switch (this) {
    MessageDeliveryStatus.delivered => "delivered",
    MessageDeliveryStatus.queued => "queued",
    MessageDeliveryStatus.acknowledged => "acknowledged",
    MessageDeliveryStatus.broadcast => "broadcast",
  };
}

enum MessageScope {
  direct,
  broadcast;

  factory MessageScope.fromJson(dynamic json) => switch (json) {
    "direct" => MessageScope.direct,
    "broadcast" => MessageScope.broadcast,
    final other => throw ArgumentError("Unknown variant: $other"),
  };

  dynamic toJson() => switch (this) {
    MessageScope.direct => "direct",
    MessageScope.broadcast => "broadcast",
  };
  @override
  String toString() => switch (this) {
    MessageScope.direct => "direct",
    MessageScope.broadcast => "broadcast",
  };
}
