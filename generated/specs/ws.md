# Edge Toolkit WebSocket Protocol 0.2.0 documentation

Edge Toolkit WS protocol -- typed et-* messages plus relay envelopes for foreign frames.

## Table of Contents

- [Servers](#servers)
  - [local](#local-server)
- [Operations](#operations)
  - [RECEIVE /ws](#receive-ws-operation)
  - [SEND /ws](#send-ws-operation)

## Servers

### `local` Server

- URL: `ws://localhost:8080/`
- Protocol: `ws`

Default ws-server bind address (mise run ws-server)

## Operations

### RECEIVE `/ws` Operation

- Operation ID: `receiveWsMessage`

Receive **one of** the following messages:

#### Message AgentMessage `AgentMessage`

- Message ID: `AgentMessage`
- Content type: [application/json](https://www.iana.org/assignments/media-types/application/json)

##### Payload

| Name               | Type   | Description                                   | Value                               | Constraints | Notes                                               |
| ------------------ | ------ | --------------------------------------------- | ----------------------------------- | ----------- | --------------------------------------------------- |
| (root)             | object | -                                             | -                                   | -           | **additional properties are allowed**               |
| from_agent_id      | string | -                                             | -                                   | -           | **required**                                        |
| message            | any    | Arbitrary JSON value (opaque to the protocol) | -                                   | -           | **required**, **additional properties are allowed** |
| message_id         | string | -                                             | -                                   | -           | **required**                                        |
| scope              | string | -                                             | allowed (`"direct"`, `"broadcast"`) | -           | **required**                                        |
| server_received_at | string | -                                             | -                                   | -           | **required**                                        |
| type               | string | -                                             | const (`"et-agent-message"`)        | -           | **required**                                        |

> Examples of payload _(generated)_

```json
{
  "from_agent_id": "string",
  "message": null,
  "message_id": "string",
  "scope": "direct",
  "server_received_at": "string",
  "type": "et-agent-message"
}
```

#### Message ConnectAck `ConnectAck`

- Message ID: `ConnectAck`
- Content type: [application/json](https://www.iana.org/assignments/media-types/application/json)

##### Payload

| Name     | Type   | Description | Value                                   | Constraints | Notes                                 |
| -------- | ------ | ----------- | --------------------------------------- | ----------- | ------------------------------------- |
| (root)   | object | -           | -                                       | -           | **additional properties are allowed** |
| agent_id | string | -           | -                                       | -           | **required**                          |
| status   | string | -           | allowed (`"assigned"`, `"reconnected"`) | -           | **required**                          |
| type     | string | -           | const (`"et-connect-ack"`)              | -           | **required**                          |

> Examples of payload _(generated)_

```json
{
  "agent_id": "string",
  "status": "assigned",
  "type": "et-connect-ack"
}
```

#### Message Invalid `Invalid`

- Message ID: `Invalid`
- Content type: [application/json](https://www.iana.org/assignments/media-types/application/json)

##### Payload

| Name       | Type           | Description | Value                  | Constraints | Notes                                 |
| ---------- | -------------- | ----------- | ---------------------- | ----------- | ------------------------------------- |
| (root)     | object         | -           | -                      | -           | **additional properties are allowed** |
| detail     | string         | -           | -                      | -           | **required**                          |
| message_id | string \| null | -           | -                      | -           | -                                     |
| type       | string         | -           | const (`"et-invalid"`) | -           | **required**                          |

> Examples of payload _(generated)_

```json
{
  "detail": "string",
  "message_id": "string",
  "type": "et-invalid"
}
```

#### Message ListAgentsResponse `ListAgentsResponse`

- Message ID: `ListAgentsResponse`
- Content type: [application/json](https://www.iana.org/assignments/media-types/application/json)

##### Payload

| Name                 | Type                | Description | Value                                     | Constraints   | Notes                                 |
| -------------------- | ------------------- | ----------- | ----------------------------------------- | ------------- | ------------------------------------- |
| (root)               | object              | -           | -                                         | -             | **additional properties are allowed** |
| agents               | array&lt;object&gt; | -           | -                                         | -             | **required**                          |
| agents.agent_id      | string              | -           | -                                         | -             | **required**                          |
| agents.last_known_ip | string              | -           | -                                         | format (`ip`) | **required**                          |
| agents.state         | string              | -           | allowed (`"connected"`, `"disconnected"`) | -             | **required**                          |
| type                 | string              | -           | const (`"et-list-agents-response"`)       | -             | **required**                          |

> Examples of payload _(generated)_

```json
{
  "agents": [
    {
      "agent_id": "string",
      "last_known_ip": "string",
      "state": "connected"
    }
  ],
  "type": "et-list-agents-response"
}
```

#### Message MessageStatus `MessageStatus`

- Message ID: `MessageStatus`
- Content type: [application/json](https://www.iana.org/assignments/media-types/application/json)

##### Payload

| Name       | Type           | Description | Value                                                                | Constraints | Notes                                 |
| ---------- | -------------- | ----------- | -------------------------------------------------------------------- | ----------- | ------------------------------------- |
| (root)     | object         | -           | -                                                                    | -           | **additional properties are allowed** |
| detail     | string         | -           | -                                                                    | -           | **required**                          |
| message_id | string \| null | -           | -                                                                    | -           | -                                     |
| status     | string         | -           | allowed (`"delivered"`, `"queued"`, `"acknowledged"`, `"broadcast"`) | -           | **required**                          |
| type       | string         | -           | const (`"et-message-status"`)                                        | -           | **required**                          |

> Examples of payload _(generated)_

```json
{
  "detail": "string",
  "message_id": "string",
  "status": "delivered",
  "type": "et-message-status"
}
```

#### Message RelayBinary `RelayBinary`

- Message ID: `RelayBinary`
- Content type: [application/octet-stream](https://www.iana.org/assignments/media-types/application/octet-stream)

##### Payload

| Name   | Type   | Description                        | Value | Constraints       | Notes |
| ------ | ------ | ---------------------------------- | ----- | ----------------- | ----- |
| (root) | string | Any binary frame, forwarded as is. | -     | format (`binary`) | -     |

> Examples of payload _(generated)_

```json
"string"
```

#### Message RelayText `RelayText`

- Message ID: `RelayText`
- Content type: [text/plain](https://www.iana.org/assignments/media-types/text/plain)

##### Payload

| Name   | Type   | Description                                                 | Value | Constraints | Notes |
| ------ | ------ | ----------------------------------------------------------- | ----- | ----------- | ----- |
| (root) | string | Any text frame that is not an et- message, forwarded as is. | -     | -           | -     |

> Examples of payload _(generated)_

```json
"string"
```

#### Message Response `Response`

- Message ID: `Response`
- Content type: [application/json](https://www.iana.org/assignments/media-types/application/json)

##### Payload

| Name    | Type   | Description | Value                   | Constraints | Notes                                 |
| ------- | ------ | ----------- | ----------------------- | ----------- | ------------------------------------- |
| (root)  | object | -           | -                       | -           | **additional properties are allowed** |
| message | string | -           | -                       | -           | **required**                          |
| type    | string | -           | const (`"et-response"`) | -           | **required**                          |

> Examples of payload _(generated)_

```json
{
  "message": "string",
  "type": "et-response"
}
```

### SEND `/ws` Operation

- Operation ID: `sendWsMessage`

Sending **one of** the following messages:

#### Message Alive `Alive`

- Message ID: `Alive`
- Content type: [application/json](https://www.iana.org/assignments/media-types/application/json)

##### Payload

| Name      | Type   | Description | Value                | Constraints | Notes                                 |
| --------- | ------ | ----------- | -------------------- | ----------- | ------------------------------------- |
| (root)    | object | -           | -                    | -           | **additional properties are allowed** |
| timestamp | string | -           | -                    | -           | **required**                          |
| type      | string | -           | const (`"et-alive"`) | -           | **required**                          |

> Examples of payload _(generated)_

```json
{
  "timestamp": "string",
  "type": "et-alive"
}
```

#### Message BroadcastMessage `BroadcastMessage`

- Message ID: `BroadcastMessage`
- Content type: [application/json](https://www.iana.org/assignments/media-types/application/json)

##### Payload

| Name    | Type   | Description                                   | Value                            | Constraints | Notes                                               |
| ------- | ------ | --------------------------------------------- | -------------------------------- | ----------- | --------------------------------------------------- |
| (root)  | object | -                                             | -                                | -           | **additional properties are allowed**               |
| message | any    | Arbitrary JSON value (opaque to the protocol) | -                                | -           | **required**, **additional properties are allowed** |
| type    | string | -                                             | const (`"et-broadcast-message"`) | -           | **required**                                        |

> Examples of payload _(generated)_

```json
{
  "message": null,
  "type": "et-broadcast-message"
}
```

#### Message ClientEvent `ClientEvent`

- Message ID: `ClientEvent`
- Content type: [application/json](https://www.iana.org/assignments/media-types/application/json)

##### Payload

| Name              | Type   | Description                                                                                                                                                                                                                                                                                    | Value                       | Constraints          | Notes                                               |
| ----------------- | ------ | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------- | -------------------- | --------------------------------------------------- |
| (root)            | object | -                                                                                                                                                                                                                                                                                              | -                           | -                    | **additional properties are allowed**               |
| event             | object | A `CloudEvents` 1.0 event in the JSON event format, as `et-client-event` carries it. `data` is always JSON, so `datacontenttype` is omitted: JSON events imply `application/json`. `time` is optional in the specification but required here: every producer has a clock, and the hub logs it. | -                           | -                    | **required**, **additional properties are allowed** |
| event.data        | any    | The event payload.                                                                                                                                                                                                                                                                             | -                           | -                    | **required**, **additional properties are allowed** |
| event.id          | string | Unique among the events of one `source`.                                                                                                                                                                                                                                                       | -                           | -                    | **required**                                        |
| event.source      | string | The producing module's served path, `/modules/<package name>`.                                                                                                                                                                                                                                 | -                           | -                    | **required**                                        |
| event.specversion | string | The `CloudEvents` specification versions a `CloudEvent` in this protocol may declare. An event declaring any other version fails to decode.                                                                                                                                                    | allowed (`"1.0"`)           | -                    | **required**                                        |
| event.time        | string | When the event happened; an RFC 3339 timestamp on the wire.                                                                                                                                                                                                                                    | -                           | format (`date-time`) | **required**                                        |
| event.type        | string | `et.<capability>.<action>`.                                                                                                                                                                                                                                                                    | -                           | -                    | **required**                                        |
| type              | string | -                                                                                                                                                                                                                                                                                              | const (`"et-client-event"`) | -                    | **required**                                        |

> Examples of payload _(generated)_

```json
{
  "event": {
    "data": null,
    "id": "string",
    "source": "string",
    "specversion": "1.0",
    "time": "2019-08-24T14:15:22Z",
    "type": "string"
  },
  "type": "et-client-event"
}
```

#### Message Connect `Connect`

- Message ID: `Connect`
- Content type: [application/json](https://www.iana.org/assignments/media-types/application/json)

##### Payload

| Name     | Type           | Description | Value                  | Constraints | Notes                                 |
| -------- | -------------- | ----------- | ---------------------- | ----------- | ------------------------------------- |
| (root)   | object         | -           | -                      | -           | **additional properties are allowed** |
| agent_id | string \| null | -           | -                      | -           | -                                     |
| type     | string         | -           | const (`"et-connect"`) | -           | **required**                          |

> Examples of payload _(generated)_

```json
{
  "agent_id": "string",
  "type": "et-connect"
}
```

#### Message ListAgents `ListAgents`

- Message ID: `ListAgents`
- Content type: [application/json](https://www.iana.org/assignments/media-types/application/json)

##### Payload

| Name   | Type   | Description | Value                      | Constraints | Notes                                 |
| ------ | ------ | ----------- | -------------------------- | ----------- | ------------------------------------- |
| (root) | object | -           | -                          | -           | **additional properties are allowed** |
| type   | string | -           | const (`"et-list-agents"`) | -           | **required**                          |

> Examples of payload _(generated)_

```json
{
  "type": "et-list-agents"
}
```

#### Message MessageAck `MessageAck`

- Message ID: `MessageAck`
- Content type: [application/json](https://www.iana.org/assignments/media-types/application/json)

##### Payload

| Name       | Type   | Description | Value                      | Constraints | Notes                                 |
| ---------- | ------ | ----------- | -------------------------- | ----------- | ------------------------------------- |
| (root)     | object | -           | -                          | -           | **additional properties are allowed** |
| message_id | string | -           | -                          | -           | **required**                          |
| type       | string | -           | const (`"et-message-ack"`) | -           | **required**                          |

> Examples of payload _(generated)_

```json
{
  "message_id": "string",
  "type": "et-message-ack"
}
```

#### Message RelayBinary `RelayBinary`

- Message ID: `RelayBinary`
- Content type: [application/octet-stream](https://www.iana.org/assignments/media-types/application/octet-stream)

##### Payload

| Name   | Type   | Description                        | Value | Constraints       | Notes |
| ------ | ------ | ---------------------------------- | ----- | ----------------- | ----- |
| (root) | string | Any binary frame, forwarded as is. | -     | format (`binary`) | -     |

> Examples of payload _(generated)_

```json
"string"
```

#### Message RelayText `RelayText`

- Message ID: `RelayText`
- Content type: [text/plain](https://www.iana.org/assignments/media-types/text/plain)

##### Payload

| Name   | Type   | Description                                                 | Value | Constraints | Notes |
| ------ | ------ | ----------------------------------------------------------- | ----- | ----------- | ----- |
| (root) | string | Any text frame that is not an et- message, forwarded as is. | -     | -           | -     |

> Examples of payload _(generated)_

```json
"string"
```

#### Message SendAgentMessage `SendAgentMessage`

- Message ID: `SendAgentMessage`
- Content type: [application/json](https://www.iana.org/assignments/media-types/application/json)

##### Payload

| Name        | Type   | Description                                   | Value                             | Constraints | Notes                                               |
| ----------- | ------ | --------------------------------------------- | --------------------------------- | ----------- | --------------------------------------------------- |
| (root)      | object | -                                             | -                                 | -           | **additional properties are allowed**               |
| message     | any    | Arbitrary JSON value (opaque to the protocol) | -                                 | -           | **required**, **additional properties are allowed** |
| to_agent_id | string | -                                             | -                                 | -           | **required**                                        |
| type        | string | -                                             | const (`"et-send-agent-message"`) | -           | **required**                                        |

> Examples of payload _(generated)_

```json
{
  "message": null,
  "to_agent_id": "string",
  "type": "et-send-agent-message"
}
```
