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

### 2. External Server → Local Delivery Only

When a message arrives from a relay server (parent or child):

```
Relay Server
       │
       ▼
fn_parseExternalMessage()
       │
       └──► Local delivery ONLY (fn_sendToAll*, fn_sendToIndividual)
       
       ✗ NO re-forwarding to other servers
```

**Code path:**
1. `ParentCommServer.on('message')` or `ChildCommServer.onReceive()`
2. Calls `fn_parseExternalMessage(p_message, p_isBinary)`
3. Checks `_origin` field for loop prevention
4. Routes to local clients only
5. **Does NOT call `forwardMessage()`** - prevents infinite loops

---

## Loop Prevention Mechanism

### Origin Tracking

Each server has a unique `server_id` defined in config:

```javascript
function getServerOriginID() {
    return global.m_serverconfig?.m_configuration?.server_id || 'unknown';
}
```

### Injection Point

In `forwardMessage()`, the `_origin` field is injected into the message (first hop only):

```javascript
if (!v_jmsg._origin) {
    v_jmsg._origin = getServerOriginID();
}
```

### Check Point

In `fn_parseExternalMessage()`, messages originating from this server are dropped:

```javascript
if (v_jmsg._origin === getServerOriginID()) {
    return;  // Ignore - this message came from us
}
```

### Why This Matters

When `ParentCommServer.forwardMessage()` broadcasts to **all** children, it includes the originating child. Without origin tracking:

```
Child A sends message → Parent → broadcasts to [A, B, C]
                                        │
                                        └─► Child A receives its own message back!
```

With `_origin` check, Child A ignores the bounce-back.

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
| Local WS Client | `fn_parseMessage()` | ✅ | ✅ | Injects `_origin` |
| Parent Server | `fn_parseExternalMessage()` | ✅ | ❌ | Checks `_origin` |
| Child Server | `fn_parseExternalMessage()` | ✅ | ❌ | Checks `_origin` |
