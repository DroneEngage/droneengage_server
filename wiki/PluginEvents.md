# Plugin Event Hooks — Design

Status: **design, not implemented**. Scope agreed 2026-10-21.

## 0. Scope

Comm server plugins today have one hook: `processCommand`, called for every
inbound client message so a plugin can analyse user traffic
(`src/comm_server/chat_server/pluginManager.js`, called from
`js_chat_routing.js:fn_parseMessage`). That hook **stays exactly as it is**.

This design adds a second, independent capability: **read-only event hooks for
the server's own events** — client connections and handshakes, the link to the
authenticator, the link to the storage server, the S2S mesh (parent/child),
UDP stream proxies, and server lifecycle.

| In scope | Out of scope |
|---|---|
| Things only the server can see: why a handshake failed, when the storage link dropped, whether a child server authenticated | Unit / fleet status (armed, battery, mode, position). That is the GCS's job; plugins do not duplicate it |
| Observing after the fact | Blocking, delaying or modifying anything (no filters, no fault injection) |
| Metadata (ids, codes, timings, sizes) | Message bodies (that is `processCommand`'s domain) and secrets |

**Primary user:** the operator of a production DE server — diagnosing "my drone
can't connect", watching infrastructure links (auth / storage / mesh), spotting
brute-force or rogue-server attempts, capacity.
**Secondary user:** the simulator, which uses the same public events as
assertions against a real server (§5). It gets no hook of its own: if an event
isn't useful to an operator it doesn't belong in the catalog.

---

## 1. Event catalog

Every entry below was checked against the code as of server v5.6.2. "Emit at"
names the exact place the emit call goes. **Bold** notes mark events that need
more than a one-line emit (a small refactor or a behaviour the code doesn't
have yet) — see §1.8 for the full list.

### 1.0 Envelope

Every event a plugin receives has the same shape (deep-frozen):

```js
{
  name: 'client.handshake.fail',   // catalog name
  ts:   1792540800123,             // Date.now() at emit
  seq:  4182,                      // per-process monotonic counter
  server_id: 'comm-hq',            // config server_id
  data: { ... }                    // per-event fields below
}
```

Common field conventions: `ip`/`port` = remote socket address; `party_id` =
senderID; `duration_ms` = since the matching open/accept; `key_hash` = first 12
hex chars of sha256 of a login temp key (never the key itself).

### 1.1 Client connections — `chat_server/js_chat_connection.js`

| Event | Emit at | Fires when | `data` |
|---|---|---|---|
| `client.connect` | `fn_onConnect_Handler`, top | A WebSocket opens, before any auth | `ip, port, auth_path` (`frame` \| `legacy`), `mode` (`local` \| `auth`) |
| `client.handshake.fail` | see reasons | Credentials refused; socket is being closed | `ip, port, auth_path, mode, reason, key_hash?` |
| `client.accepted` | `_acceptConnection`, after group join | Party admitted (both local and auth modes) | `ip, port, party_id, actor_type, account_id, group_id, mode, auth_path, handshake_ms` |
| `client.replaced` | `_acceptConnection`, before `fn_del_member_fromAccountByName(.., true)` | A new connection with an existing senderID kicks out the old socket | `party_id, old_ip, old_port, new_ip, new_port, old_duration_ms` |
| `client.close` | `fn_onWsClose` | Socket closed, authenticated or not | `ip, port, party_id?, code, authed, replaced, duration_ms, ttx_bytes, dropped_msgs` |
| `client.outbound_terminated` | `js_outbound_guard.js`, hard-cap branch (`RESULT_TERMINATED`) | Server killed a socket whose send buffer exceeded `hard_cap_bytes` | `party_id, buffered_bytes, hard_cap_bytes` |

`client.handshake.fail` reasons and where each is decided:

| `reason` | Where |
|---|---|
| `legacy_rejected` | `fn_onConnect_Handler`, query creds while `ws_auth_no_frame_old_compatibility: false` |
| `frame_timeout` | auth-frame `setTimeout` (8 s) |
| `frame_not_json` | `fn_authFrameHandler`, JSON.parse catch |
| `frame_not_de_auth` | `fn_authFrameHandler`, `ty`/`mt` check |
| `key_missing` / `key_bad_format` | `fn_validateKey` (null params / non-alphanumeric or > 200 chars) |
| `key_unknown` | `fn_validateKey`, `!isLoginExist` — unknown, already used, or expired (> 10 s) |
| `validation_error` | `fn_authFrameHandler`, catch around `fn_completeConnection` |

**Refactor needed:** `fn_completeConnection` ignores `fn_validateKey`'s return
value and calls `acceptConnection` anyway (which then closes a second time). The
emit has to sit in `fn_validateKey`, and its return value should gate
`acceptConnection` so one failure produces exactly one event.

`client.connect`/`client.close` fire for **every** socket, including scanners
that never authenticate — that's intended (security visibility), and why
`authed` is in `client.close`.

### 1.2 Authenticator link — `js_comm_server_manager_client.js`, `js_andruav_comm_server.js`

| Event | Emit at | Fires when | `data` |
|---|---|---|---|
| `auth.link.open` | `fn_onOpen_Handler` | WSS to authenticator open | `host, port` |
| `auth.link.close` | `fn_onClose_Handler` | Link closed; retry is scheduled | `host, port, retry_ms` (2000), `queue_discarded` |
| `auth.link.error` | `fn_onError_Handler` | Socket error (precedes close) | `host, port, error` (message string) |
| `auth.challenge.answered` | `fn_onMessage_Handler`, after `send(fn_buildResponse)` | Comm server signed the authenticator's S2S challenge | `method` (`ed25519`) |
| `auth.challenge.sign_failed` | `fn_onMessage_Handler`, catch | Could not sign (bad/missing `s2s_my_private_key`); link is closed | `error` |
| `auth.queue.flushed` | `fn_flushQueue` | Queued messages sent after (re)connect; only when count > 0 | `count` |
| `auth.login.offered` | `js_andruav_comm_server.js:fn_handleLoginResponses` | Authenticator asked for a login slot; temp key issued | `party_id, actor_type, request_id, key_hash` |
| `auth.login.expired` | `fn_addWaitingAccount` timeout callback | Temp key reached 10 s unused | `party_id, key_hash` |
| `admin.unit_traffic` | `fn_handleSetUnitTrafficState` | Admin (via authenticator) blocked/unblocked a unit's inbound traffic | `name, blocked, applied` |
| `admin.udp_proxy` | `fn_handleSetUdpProxyState` | Admin blocked/unblocked a UDP proxy | `name, blocked, applied` |

**Can't be supported as drafted:** "auth handshake ok". The comm server only
*answers* the challenge; the authenticator never says whether it accepted the
signature (a rejected comm server is just disconnected → `auth.link.close`).
So there's `auth.challenge.answered`, not `auth.challenge.ok`.

**Refactor needed for `auth.login.expired`:** the timeout callback calls
`deleteLogin` unconditionally; it has to check `isLoginExist` first so only an
*unused* key emits.

**Bug spotted (separate task):** `fn_sendMessage` queues without limit while
`m_ws` exists but isn't open — the `m_ws || length < 1000` condition is always
true after the first connect attempt. `auth.queue.flushed.count` will show it.

### 1.3 Storage link — `server_to_server/js_db_proxy_client.js`

| Event | Emit at | Fires when | `data` |
|---|---|---|---|
| `storage.link.connecting` | `fn_connectInternal` | Dial attempt started | `host, port, attempt` |
| `storage.link.open` | `fn_onOpen` | WSS open; S2S auth now pending | `host, port` |
| `storage.auth.ok` | `fn_onMessage`, `type === 'auth_success'` | Storage server accepted us; link usable | `host, port, method, auth_ms` |
| `storage.auth.sign_failed` | `fn_onMessage`, challenge catch | Could not build the response; link closed | `method, error` |
| `storage.auth.timeout` | `m_s2sAuthTimer` callback | No `auth_success` within 8 s; closing (fail-closed) | `host, port` |
| `storage.link.close` | `fn_onClose` | Link closed | `host, port, was_authed, pending_failed, attempt, retry_ms` |
| `storage.link.error` | `fn_onError` | Socket error (precedes close) | `host, port, error` |
| `storage.server_error` | `fn_onMessage`, `type === 'error'` | Storage server returned a protocol-level error | `error` |
| `storage.request` | `fn_sendRequest`, on settle | One request finished, in any way | `mt, op, result, latency_ms, success?` |

`storage.request.result` ∈ `ok` (response received; `success` copied from the
envelope), `timeout` (10 s), `disconnected` (rejected by `fn_clearPending` on
close), `not_connected` (rejected before sending), `send_error`. `op` is the
symbolic name (`SaveMission`, `LoadTasks`, …) mapped from `mt`.

Left out on purpose: the 30 s heartbeat and the `CONST_CS_CMD_STORAGE_STATUS`
reports to the authenticator. They repeat what the link/auth events already
say. `NewsPush` from storage is content, not infrastructure.

### 1.4 S2S mesh, parent side — `server_to_server/js_parent_comm_server.js`

| Event | Emit at | Fires when | `data` |
|---|---|---|---|
| `s2s.listen` | `_startServer`, `server.listen` callback | Super-server port is listening | `host, port` |
| `s2s.child.connect` | `wss.on('connection')` | A child opened a socket; challenge sent | `ip, port` |
| `s2s.child.auth_ok` | message handler, verify success | Child proved its key | `ip, port, child_id, auth_ms` |
| `s2s.child.auth_fail` | message handler else-branch / auth timer | Child rejected; socket terminated | `ip, port, child_id?, reason` |
| `s2s.child.close` | `child_ws.on('close')` | Child socket closed | `ip, port, child_id?, authed, duration_ms` |
| `s2s.error` | `wss.on('error')` | Listener error | `error` |

`s2s.child.auth_fail.reason` ∈ `timeout`, `rejected`. **Refactor needed** to
split `rejected` into `unknown_server_id` / `bad_signature` / `malformed`:
`js_s2s_auth.fn_verify` returns only a boolean. A variant that also returns the
reason (keeping `fn_verify`'s signature) is enough.

### 1.5 S2S mesh, child side — `server_to_server/js_child_comm_server.js`

Named `s2s.uplink.*` (child → its parent) so the name says which side you're
on; the draft's `s2s.parent.link.*` read as the parent's own events.

| Event | Emit at | Fires when | `data` |
|---|---|---|---|
| `s2s.uplink.connecting` | `connectToParent` | Dial attempt | `host, port` |
| `s2s.uplink.open` | `parentWs.on('open')` | Socket open | `host, port` |
| `s2s.uplink.challenge_answered` | `onReceive`, after sending the response | We signed the parent's challenge | `method` (`ed25519`) |
| `s2s.uplink.close` | `parentWs.on('close')` | Uplink closed; retry scheduled | `host, port, code, reason, duration_ms, retry_ms` (10000) |
| `s2s.uplink.error` | `parentWs.on('error')` | Socket error | `host, port, error` |
| `s2s.uplink.dropped` | `forwardMessage`, not-connected branch | **Coalesced:** messages discarded because the uplink was down | `count, window_ms` |

**Can't be supported as drafted:** "child authenticated to parent". The parent
sends nothing on success, so a child can't tell an accepted handshake from one
still in flight; a rejection shows up only as `s2s.uplink.close`. Options: keep
`challenge_answered` (no server behaviour change), or have the parent send
`{s2s_auth:'ok'}` after verification (small protocol addition; old children
ignore it because `onReceive` drops unknown envelopes). **Decision: added in
phase 3.** Then `s2s.uplink.auth_ok` exists, and the sim can wait for
"mesh formed" on either side.

### 1.6 Relay (both roles) — `chat_server/js_chat_routing.js`, `js_chat_relay.js`

| Event | Emit at | Fires when | `data` |
|---|---|---|---|
| `s2s.loop_dropped` | `fn_parseExternalMessage`, `_path` includes own id | **Coalesced:** relayed messages discarded by loop prevention | `count, window_ms` |
| `s2s.relay_parse_failed` | `forwardMessage` / `forwardExternalMessage` catch | **Coalesced:** message forwarded without relay metadata | `count, window_ms` |

**Coalesced** events never fire per message. The emit site bumps a counter, and
the bus flushes `{count, window_ms}` at most once per second while the count is
non-zero. That keeps the hot path at one integer increment.

### 1.7 UDP stream proxies — `js_udp_proxy.js`

| Event | Emit at | Fires when | `data` |
|---|---|---|---|
| `udp.proxy.created` | `getUDPSocket`, new-proxy callback | Proxy pair created for a unit | `name, port1, port2, ready` |
| `udp.proxy.closed` | `closeUDPSocket` | Proxy closed | `name, reason, age_ms` |
| `udp.proxy.recreated` | `recreateProxy` completion | Rebuilt (health check, resume, refresh) | `name, reason, ok` |
| `udp.proxy.reaped` | `startReaper`, idle branch | Idle proxy of a disconnected unit closed | `name, idle_ms` |
| `udp.proxy.bind_failed` | `udp_socket` `'error'` with `EADDRINUSE` | A requested port is taken | `name, port` |
| `server.resume` | `recoverAllProxiesAfterResume` | Timer gap > 3× reaper interval (host was suspended) | `gap_ms, proxies` |

**Refactor needed:** `closeUDPSocket` gets an optional `reason` argument
(`unit_request` from `js_chat_system_commands.js`, `unit_disconnect` from
`fn_onWsClose`, `reconfigure` from the recursive call in `getUDPSocket`).
`udp_socket` has no proxy name today; pass it in from `udp_proxy`.

**Limitation (flag):** `server.resume` is detected only while the UDP reaper is
running, i.e. while at least one proxy exists. A general detector would be a
gap check in `server.js` (the existing 60 s `checkMemory` interval would do).
Recommend moving detection there in phase 3. The reaper keeps calling
`recoverAllProxiesAfterResume`.

### 1.8 Server lifecycle — `server.js`, `chat_server/js_chat_connection.js`

| Event | Emit at | Fires when | `data` |
|---|---|---|---|
| `server.start` | `server.js:fn_startServer`, end | Config loaded, subsystems started | `version, server_id, port, roles{super, relay, storage, local_mode, auth}, plugins[], warnings[]` |
| `server.listening` | `fn_startChatServer`, `wserver.listen` callback | Client WSS port is accepting | `ip, port` |
| `server.wss_error` | `fn_startChatServer`, `v_wss.on('error')` | Client listener error | `error` |
| `server.stop` | SIGINT handler; `checkMemory` exit | Process is about to exit | `reason` (`sigint` \| `memory_limit`), `rss_mb?` |
| `plugin.disabled` | plugin host | A plugin was auto-disabled (§3) | `plugin, reason, errors` |
| `plugin.overflow` | plugin host | A plugin's queue overflowed; events dropped | `plugin, dropped` |

`server.start.warnings` carries startup issues that are currently only console
text, e.g. the UDP kernel-buffer check from `checkAndFixKernelBuffers`.

**Refactor needed:** both exit paths call `process.exit` immediately. They need
to emit `server.stop`, run plugin `shutdown()` (2 s total budget), then exit.

### 1.9 Changes from the original draft

| Draft | Outcome |
|---|---|
| connect, handshake ok/fail, accepted, close/error | Kept. `handshake.ok` merged into `client.accepted`; added `client.replaced` and `client.outbound_terminated` |
| auth link open/close/error, auth.request, auth.queue | Kept. `auth.request` became `auth.login.offered` / `auth.login.expired` (the comm server never sees the authenticator's verdict on the *user's* credentials) |
| storage link, s2s_auth ok/fail, storage.request, storage.status | Kept, except `storage.status` (duplicate) |
| s2s child connected/disconnected/auth_fail; parent link open/close/error; challenge ok/fail | Kept, renamed to `s2s.child.*` / `s2s.uplink.*`. **Child-side "auth ok" isn't possible without the protocol addition in §1.5**; same for the authenticator link (§1.2) |
| udp proxy created/closed/recreated/reaped/blocked; server.resume | Kept; `blocked` moved to `admin.udp_proxy` because it's an admin action. **`server.resume` is limited** until detection moves (§1.7) |
| server start/stop, wss error | Kept |

---

## 2. Plugin contract

### 2.1 Configuration

```jsonc
// server.config
"command_plugin": ["../droneengage_server_plugins/aisstream/index.js"],  // legacy, unchanged
"plugins": [
  { "path": "../droneengage_server_plugins/event_log", "enabled": true,
    "config": { "file": "./logs/events.jsonl", "subscribe": ["client.*", "storage.*"] } }
]
```

`path` resolves exactly like `command_plugin` (relative to the server root,
absolute allowed). `config` is handed to the plugin as-is. Secrets stay in the
plugin's own `.env`, as `aisstream` does today.

### 2.2 Module shape

```js
module.exports = {
  api: 1,                       // host API version the plugin was written for
  name: 'event_log',
  version: '1.0.0',
  init(api, config) { ... },    // sync or async; subscribe here
  shutdown(reason) { ... },     // optional; clear timers, flush files
  processCommand(...) { ... },  // optional; same legacy message hook, same args
};
```

The host refuses to load a plugin whose `api` major is higher than its own,
and logs exactly why.

### 2.3 Host `api` object (v1)

| Member | Purpose |
|---|---|
| `api.version` | `1` |
| `api.server` | frozen `{ id, version, roles }` |
| `api.on(pattern, handler)` | Subscribe; returns an `unsubscribe()` function |
| `api.events()` | The catalog names this server can emit (for validation / UIs) |
| `api.log.info/warn/error(msg, obj?)` | Server logger, prefixed `[plugin:<name>]`; replaces `require('../../comm_server/js_dumperror.js')` |
| `api.constants` | Frozen copy of the public protocol constants (`CONST_WS_*`, message type ids); replaces copying them into the plugin as `aisstream` does |
| `api.hash(value)` | The same 12-hex sha256 the host uses, so plugins can correlate `key_hash` values |
| `api.config` | The plugin's own `config` block (frozen) |

Patterns: an exact name (`client.close`), a prefix wildcard (`storage.*`,
`s2s.*`, matching one or more segments), or `*`. Subscribing to a pattern that
matches nothing in `api.events()` logs a warning (typo guard) but is allowed.

Deliberately **not** in v1: sending messages, reading sockets, timers or
storage helpers. v1 is observe-only. Message-sending plugins (aisstream)
keep using the `p_ws` they get through `processCommand`.

### 2.4 Backward compatibility

- `command_plugin` and `fn_processPlugins` don't change (still fire-and-forget,
  same five arguments). Existing deployments behave the same.
- A module listed in both `command_plugin` and `plugins` is loaded once (deduped
  by resolved path).
- A `plugins` entry without `init` but with `processCommand` is treated like a
  legacy plugin.
- `aisstream` and `command_processing` move to `api: 1` in phase 5. Until then
  they work untouched.

---

## 3. Rules

1. **Observe only.** Handlers can't influence the server: no return value is
   read, payloads are deep-frozen copies, and no `ws`, socket or internal
   object is ever passed.
2. **Async, after the fact.** The emit site enqueues; handlers run later via
   `setImmediate`, never inside the emitter's call stack. Order is preserved
   per plugin (`seq`).
3. **Metadata only.** No message bodies, no S2S nonces or signatures, no key
   material or key paths. Login temp keys appear only as `key_hash`. Error
   strings are `err.message`, not stacks. IPs are included by default (an
   operator's own server, needed for security), with a host option
   `plugin_events_redact_ip: true` that replaces them with a hash.
4. **Errors contained.** Each handler call (and each returned promise) is
   wrapped. Per plugin, more than 50 errors in 60 s → the plugin is disabled,
   one console warning, `plugin.disabled` goes to the other plugins, and the
   server keeps running.
5. **Bounded.** Each plugin has a queue (default 10 000 events). On overflow the
   oldest events are dropped and counted, and `plugin.overflow` is reported once
   per second. A slow plugin can't grow the heap.
6. **Zero cost when unused.** Emit sites are written as
   `if (bus.wants('client.close')) bus.emit('client.close', () => ({...}))`.
   `wants()` is a single `Map` lookup against a table rebuilt only when
   subscriptions change. The payload builder (the lambda) runs only if someone
   subscribed. Coalesced counters (§1.6) are one integer increment.
7. **Shutdown.** On `server.stop` the host calls every `shutdown()` in parallel
   and exits after they settle or after 2 s, whichever comes first.

---

## 4. Where the plugin host lives

**Decision (2026-10-21): the plugin host stays in the comm server for now,
next to the existing message plugin.** Moving to `droneengage_server_common` is
a separate, later task (phase 7), and it moves **all** plugin infrastructure
together, not just the event bus.

Now (phases 1–5), in `src/comm_server/plugins/`:
- the existing `pluginManager.js` (message hook, `command_plugin`) moves into
  this folder unchanged, as part of the same host;
- the new parts go next to it: loader for `plugins`, API version check, bus
  (`wants`/`emit`/pattern table/coalescing), per-plugin queues, isolation and
  auto-disable, shutdown, the `api` object;
- the comm server catalog (names for `api.events()` and the typo guard).

Keep the line between the generic parts and the comm-server parts clean from
the start: the generic files never `require` comm-server modules, and
everything server-specific (catalog, `api.server`, `api.constants`) is passed
in when the host is created. Then the later move is a file move plus a
dependency bump, not a rewrite.

Later (phase 7), move to `droneengage_server_common` (`lib/plugins/`): the whole
host, message hook and event bus, so the authenticator and storage server can
adopt the same contract (phase 8). Common is consumed by git tag (`#v1.0.1`) and
vendored on deploy (see the de-cloud-deploy skill), so the move ships as a
common tag bump plus a dependency bump in each server.

---

## 5. Simulator use

The sim gets visibility into the real server without the fake server: it can
see which units the server admitted, why a dial failed, and when the mesh,
storage and auth links went down and came back. This is a **partial** stand-in
for the fake server's `list`/`stats`. It doesn't inject faults; `link_proxy`
still does that.

### 5.1 The sim plugin

`droneengage_studio/engine/server_plugins/de_sim_events/` is an
ordinary `api: 1` plugin. It lives in the sim repo because it's sim-specific,
and it uses only the public API.

- `gen_server` adds it to each spawned server's `plugins` with
  `config: { file: var/run/<sim>/server/<server>/events.jsonl, port: <alloc> }`
  and an absolute `path`.
- **File (primary):** one JSON line per event, subscribed to `*`, appended
  through a write stream and flushed on `shutdown`. This is run evidence: it
  survives a crash and is read after the timeline, like the journals.
- **Port (optional):** a local TCP line stream of the same events for live
  waits during a timeline step. It's bound only when `port` is set, and the
  port comes from the port allocator (task 2), named `<server>.events`.

### 5.2 Sim-side pieces (task 3 / phase 4)

- The runner loads every server's `events.jsonl` into `ctx["server_events"]`
  (keyed by server name), next to `link_events`/`proc_events`.
- Graders in `engine/checks.py`, following `docs/CHECKS.md`:
  `server_event` (count of `name` with field filters, using `_cmp`),
  `server_event_order` (A before B), `server_event_within` (B within N ms of A
  or of a mark), `no_server_event`.
- Timeline step `wait_server_event: {server, name, where, timeout}` using the
  live port.
- Readiness: `gen_server` launch can wait for `server.listening` (and
  `s2s.listen` on a parent) instead of probing ports.

### 5.3 Events task 3 depends on

| Need in task 3 | Events |
|---|---|
| Server up and ready | `server.start`, `server.listening`, `s2s.listen` |
| Unit X landed on server Y (`via:` correctness) | `client.accepted` (`party_id`), `client.handshake.fail` |
| Reconnect after a `link_proxy` cut | `client.close` → `client.accepted` for the same `party_id`, `client.replaced` |
| Mesh formed / partitioned / healed | parent `s2s.child.auth_ok`, `s2s.child.close`; child `s2s.uplink.open`, `s2s.uplink.close`, `s2s.uplink.auth_ok` (needs the §1.5 addition) |
| Traffic lost while partitioned | `s2s.uplink.dropped` |
| Loop prevention in a multi-level mesh | `s2s.loop_dropped` |
| Mission storage works | `storage.auth.ok`, `storage.request` (`op: SaveMission/LoadMission`, `result: ok`, `latency_ms`) |
| Storage outage | `storage.link.close`, `storage.request` (`disconnected`/`not_connected`), `storage.auth.ok` again |
| `auth: true` sims | `auth.link.open`, `auth.login.offered` → `client.accepted` (match on `key_hash`/`party_id`), `auth.login.expired` |

### 5.4 Facts from this survey that task 3 must respect

- **Mesh S2S is Ed25519-only.** The parent sends no `method` and verifies with
  `fn_verify`; the child always answers with `fn_buildResponse`. HMAC shared
  secrets work toward storage (and the authenticator), **not** between comm
  servers. The parent needs `s2s_trusted_server_keys[<child server_id>]` →
  the child's public key; the child needs `s2s_my_private_key`.
- **Every mesh node needs a unique `server_id`.** Loop prevention puts
  `server_id` in `_path` and drops anything already carrying its own id.
  Two nodes with the same id silently drop each other's traffic, which shows
  up as `s2s.loop_dropped`.
- **The parent resolves relative SSL paths from a different base**
  (`__dirname + "../"` = `src/comm_server/`) than the client listener does (repo
  root). Generated configs should use **absolute** `ssl_key_file`/`ssl_cert_file`.
- Child reconnect is a fixed 10 s; the storage link backs off 1 s → 60 s with
  ±25 % jitter. Timeouts in `server_event_within` assertions must allow for that.

---

## 6. Phased implementation plan

Each phase ships finished and tested before the next one starts.

### Phase 1 — Host and lifecycle (comm server)
- `src/comm_server/plugins/`: the existing `pluginManager.js` moved in
  unchanged, plus the new loader, version check, bus, pattern table,
  coalescing, queues, isolation, shutdown and `api` object. Generic files
  don't `require` comm-server modules (§4).
- `plugins` config, catalog file, `server.start`,
  `server.listening`, `server.stop` (with the graceful exit refactor),
  `server.wss_error`, `plugin.*`.

Acceptance:
- Unit tests for the host: pattern matching (exact/prefix/`*`), `wants()` false →
  builder never called, throwing handler → auto-disable after threshold with
  others unaffected, overflow drops oldest and reports, shutdown budget enforced,
  `api: 2` plugin refused with a clear log.
- The comm server with the existing `aisstream` `command_plugin` config behaves
  the same (existing `npm test` passes; AIS sub/unsub still works).
- A test plugin receives `server.start` then `server.listening`; SIGINT delivers
  `server.stop` and the process exits within 2 s.

### Phase 2 — Client and authenticator events, plus the first operator plugin
- §1.1 and §1.2 events, including the `fn_validateKey` gating and the
  `auth.login.expired` check.
- `droneengage_server_plugins/event_log`: writes subscribed events as JSONL,
  with optional daily rotation. The reference operator plugin.

Acceptance:
- Integration tests with a real ws client cover every `client.handshake.fail`
  reason, `client.accepted` in local and auth mode, `client.replaced`, and
  `client.close` with `authed: false` for a never-authenticated socket.
- No event payload contains a raw temp key (the test generates keys and greps
  all captured payloads).
- One failed handshake produces exactly one `client.handshake.fail`.
- With no plugins configured, the routing benchmark shows no measurable change.

### Phase 3 — Storage, S2S, relay and UDP events
- §1.3–§1.7, the `fn_verify` reason variant, `closeUDPSocket` reasons,
  moving resume detection to `server.js`, and the parent → child `{s2s_auth:'ok'}`
  notice with `s2s.uplink.auth_ok`.

Acceptance:
- Mock storage server tests: `storage.auth.ok`, `storage.auth.timeout` (peer
  never confirms), and `storage.request` for each `result` value.
- In-process parent and child tests: `s2s.child.auth_ok`, `s2s.child.auth_fail`
  for unknown id / bad signature / timeout, uplink close → reconnect, an old child
  ignoring the new ok notice, `s2s.uplink.dropped` coalesced to ≤ 1 event/s under
  a burst, `s2s.loop_dropped` on a looped message.
- `server.resume` fires on a simulated timer gap with zero UDP proxies.

### Phase 4 — Sim plugin and graders (simulation repo; part of task 3)
- `de_sim_events` plugin, `gen_server` wiring, `ctx["server_events"]`,
  the graders, `wait_server_event`.

Acceptance:
- A real-server sim run writes `events.jsonl` for every spawned server.
- An example scenario asserts `client.accepted` for each unit; a `link_proxy`
  cut scenario asserts `client.close` then `client.accepted` for the same unit
  within the de_comm reconnect window.
- A storage scenario asserts `storage.request{op: SaveMission, result: ok}`
  and then a `LoadMission` returning the mission.

### Phase 5 — Migrate the existing plugins and document
- `aisstream` and `command_processing` move to `api: 1` (no `require` into
  server internals, constants from `api.constants`, logging through `api.log`).
- README "Plugins" section; `droneengage_server_plugins/readme.md` updated with
  the new contract and the event catalog link.

Acceptance:
- Neither plugin `require`s anything under the server's `src/`.
- AIS works end to end with a GCS.

### Phase 6 (nice to have) — Runtime enable/disable
Turn a plugin on or off without restarting, e.g. from the authenticator's admin
view. Needs `shutdown()` to fully release the plugin and the loader to support
loading it again. Not designed in detail here.

### Phase 7 (later) — Move the whole plugin host to `droneengage_server_common`
Move **all** plugin infrastructure, the message hook (`command_plugin` /
`processCommand`) and the event host, into `droneengage_server_common`
(`lib/plugins/`). Tag common, bump the dependency in the comm server.

Acceptance:
- No plugin code left under the comm server's `src/` except its catalog and the
  emit calls.
- Existing plugins and every phase 1–5 test pass unchanged.

### Phase 8 (later) — Authenticator and storage server adopt the host
Separate catalogs (login ok/fail, comm server registered/lost; DB errors, comm
server auth). Needs phase 7. Not designed here.

---

## 7. Decisions (answered 2026-10-21)

1. **Host location:** stays in the comm server, next to the existing message
   plugin. Moving all of it to `droneengage_server_common` is phase 7.
2. **IP addresses:** included by default; `plugin_events_redact_ip: true`
   hashes them.
3. **Mesh success notice:** confirmed for phase 3: the parent (super server) sends
   `{s2s_auth:'ok'}` to a child comm server after verifying it, so the child
   knows it was accepted (`s2s.uplink.auth_ok`). Graceful exit on SIGINT stays
   in phase 1.
4. **Runtime enable/disable:** nice to have, phase 6.

## 8. Defects found while surveying (not fixed here)

- `js_comm_server_manager_client.js:fn_sendMessage`: the authenticator queue is
  unbounded once a socket object exists (`m_ws || length < 1000`).
- `js_andruav_comm_server.js:fn_cancelLoginRequestBySenderID` references
  `c_CommServerManagerClient`, which isn't defined in that file (it's
  `m_commServerManagerClient`). Currently unreachable, but it would throw.
- `js_parent_comm_server.js`: relative SSL paths resolve from `src/comm_server/`,
  unlike the client listener (repo root).
- `js_parent_comm_server.js` / `js_child_comm_server.js`: every relayed message
  is `console.log`ged in full (`SUPER SRV RX::`, `CHILD RX:`). That's noise and
  a cost on a busy mesh.
- `js_chat_connection.js:fn_completeConnection` ignores `fn_validateKey`'s
  result (§1.1).
