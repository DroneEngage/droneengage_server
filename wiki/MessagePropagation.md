# Message Propagation in DroneEngage Communication Server

This document describes the message routing and propagation logic in the server-to-server mesh relay system.

## Architecture Overview

```
                    ┌─────────────────────┐
                    │    Super Server     │
                    │  (ParentCommServer) │
                    └──────────┬──────────┘
                               │
              ┌────────────────┼────────────────┐
              │                │                │
              ▼                ▼                ▼
       ┌────────────┐   ┌────────────┐   ┌────────────┐
       │  Child A   │   │  Child B   │   │  Child C   │
       │(ChildComm) │   │(ChildComm) │   │(ChildComm) │
       └─────┬──────┘   └─────┬──────┘   └─────┬──────┘
             │                │                │
        Local Units      Local Units      Local Units
        (WS Clients)     (WS Clients)     (WS Clients)
```

## Key Components

### Files

| File | Role |
|------|------|
| `js_andruav_chat_server.js` | Main message routing logic |
| `js_parent_comm_server.js` | Super server - accepts child connections |
| `js_child_comm_server.js` | Child server - connects to parent |

### Core Functions

| Function | Location | Purpose |
|----------|----------|---------|
| `fn_parseMessage()` | chat_server | Handles messages from **local** WebSocket clients |
| `fn_parseExternalMessage()` | chat_server | Handles messages from **relay** servers |
| `forwardMessage()` | chat_server | Forwards messages to parent/child relay servers |
| `forwardExternalMessage()` | chat_server | Re-forwards broadcast relay messages through the relay tree |
| `getServerOriginID()` | chat_server | Returns unique server ID for loop prevention |

---

## Data Flow

### 1. Local Client → Relay Propagation

When a local WebSocket client sends a message:

```
Local Client (WS)
       │
       ▼
fn_parseMessage()
       │
       ├──► Local delivery (fn_sendToMyGroup*, fn_sendToTarget)
       │
       └──► forwardMessage()
                 │
                 ├──► Parent Server (if enable_super_server=true)
                 │
                 └──► Child Servers (if enable_persistant_relay=true)
```


**Code path:**
1. `fn_onWsMessage()` receives WS message
2. Calls `fn_parseMessage(p_ws, p_message, p_isBinary)`
3. Parses JSON, injects permission (`p` field)
4. Routes to local clients based on `ty` (routing type) and `tg` (target)
5. Calls `forwardMessage()` to propagate to relay servers


### 2. External Server → Local Delivery and Relay Propagation

When a message arrives from a relay server (parent or child):

```
Relay Server
       │
       ▼
fn_parseExternalMessage()
       │
       ├──► Local delivery (fn_sendToAll*, fn_sendToIndividual)
       │
       └──► forwardExternalMessage() for broadcast-style messages
                 │
                 ├──► Parent Server (if enable_persistant_relay=true)
                 │
                 └──► Child Servers except source child (if enable_super_server=true)
```

**Code path:**
1. `ParentCommServer.on('message')` or `ChildCommServer.onReceive()`
2. Calls `fn_parseExternalMessage(p_message, p_isBinary, p_source_ws)`
3. Checks `_path` trail for loop prevention
4. Routes to local clients based on `ty` and `tg`
5. Calls `forwardExternalMessage()` only for broadcast-style relay messages
6. One-to-one external messages are delivered locally if the target exists and are not propagated further

---

## Loop Prevention Mechanism

### Path Tracking

Each server has a unique `server_id` defined in config:

```javascript
function getServerOriginID() {
    return global.m_serverconfig?.m_configuration?.server_id || 'unknown';
}
```

### Injection Point

In `forwardMessage()` and `forwardExternalMessage()`, every server appends its own ID to the `_path` trail before relaying (on EACH hop, not just the first):

```javascript
if (!Array.isArray(v_jmsg._path)) v_jmsg._path = [];
v_jmsg._path.push(getServerOriginID());
```

### Check Point

In `fn_parseExternalMessage()`, messages this server has already relayed are dropped:

```javascript
if (Array.isArray(v_jmsg._path) && v_jmsg._path.includes(getServerOriginID())) {
    return;  // Ignore - this server is already in the relay path
}
```

### Why This Matters

A single first-hop origin tag is not enough for multi-child or multi-level trees: an intermediate node does not recognize traffic it already forwarded, so a child re-forwarding a parent broadcast back up causes the parent to reprocess and amplify it indefinitely.

```
Child A sends message → Parent → broadcasts to [A, B, C]
                                        │
                                        └─► B re-forwards up → Parent reprocesses → loop!
```

Because every relay hop appends its `server_id` to `_path`, any server that sees its own ID in the trail drops the message. Combined with source-child exclusion (`exclude_ws`), this prevents both bounce-back and multi-level loops.

---

## Account/Group Propagation

### Purpose

When messages are forwarded through the relay tree, the sender's WebSocket context is lost on the receiving server. To preserve account/group isolation, the sender's account ID (`_aid`) and group ID (`_gid`) are injected into the message during forwarding. Both are required because group IDs are not unique across accounts (`c_accounts[accountId].m_groups[groupId]`).

### Injection Point

All relay metadata (`_path`, `_gid`, `_aid`) is injected in a **single parse pass** inside `forwardMessage()` in `js_andruav_chat_server.js`, using the local sender's socket group. The relay transports (`js_parent_comm_server.js` / `js_child_comm_server.js`) then forward the buffer as-is without re-parsing:

```javascript
const c_group = p_ws.m__group;
// ... append _path ...
if (c_group != null) {
    v_jmsg._gid = c_group.m_ID;
    v_jmsg._aid = c_group.m_parentAccount.m_accountID;
}
```

### Validation Point

In `fn_sendTIndividualId()` in `js_andruav_chat_account_rooms.js`, when a message arrives from a super server (senderSocket is null), the injected groupID is used for validation:

```javascript
if (senderSocket === null && groupID !== undefined) {
    // Message from super server - validate using injected groupID
    if (socket.m__group && socket.m__group.m_ID === groupID) {
        socket.send(message, { binary: isBinary });
    }
}
```

### Why This Matters

Without groupID propagation, messages from super servers would fail group validation because the sender's WebSocket is not tracked in the local `c_activeSenders`. The injected `_gid` allows the receiving server to verify that the target is in the same group as the original sender.

### Scoped Broadcasts

Relayed broadcasts (`fn_parseExternalMessage`) are delivered through `deliverExternalBroadcast()`, which resolves `c_accounts[_aid].m_groups[_gid]` via `fn_sendToAccountGroup()` and only broadcasts within that account/group. If `_aid`/`_gid` are missing (legacy messages) or the account/group is not present locally, it falls back to the legacy all-accounts broadcast. This prevents cross-account/cross-group leakage that the older `fn_sendToAll*` flooding caused.

---

## Message Routing Types

The `ty` field determines routing behavior:

| Value | Constant | Behavior |
|-------|----------|----------|
| `'g'` | `CONST_WS_MSG_ROUTING_GROUP` | Broadcast to group |
| `'i'` | `CONST_WS_MSG_ROUTING_INDIVIDUAL` | Targeted delivery |
| `'s'` | `CONST_WS_MSG_ROUTING_SYSTEM` | System commands (local only) |

### Target Field (`tg`) Special Values

| Value | Constant | Meaning |
|-------|----------|---------|
| `'_GCS_'` | `CONST_WS_SENDER_ALL_GCS` | All GCS units |
| `'_GD_'` | `CONST_WS_SENDER_ALL` | All units (GCS + Agents) |
| `'_AGN_'` | `CONST_WS_SENDER_ALL_AGENTS` | All drone agents |
| *(other)* | - | Specific unit ID (one-to-one) |

---

## Configuration Flags

In server config file:

| Flag | Effect |
|------|--------|
| `enable_super_server: true` | This server acts as a parent (accepts child connections) |
| `enable_persistant_relay: true` | This server connects to a parent as a child |
| `server_id` | Unique identifier for loop prevention |
| `s2s_ws_target_ip` / `s2s_ws_target_port` | Parent server address (for child mode) |

---

## Summary Table

| Message Source | Handler | Local Delivery | Relay Forward | Loop Check |
|----------------|---------|----------------|---------------|------------|
| Local WS Client | `fn_parseMessage()` | ✅ | ✅ | Appends to `_path` |
| Parent Server | `fn_parseExternalMessage()` | ✅ | Broadcast-style only | Checks `_path` |
| Child Server | `fn_parseExternalMessage()` | ✅ | Broadcast-style only | Checks `_path`; excludes source child when forwarding to children |
