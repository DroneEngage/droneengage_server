# UDP Relay Server — Design

Status: **implemented** in `droneengage_udp_relay_server` (TASK-50, 51, 53,
55 and 56) and in the comm server (TASK-52, 54 and 57). The de_mavlink,
webclient, authenticator and simulator parts follow in their own tasks. Where
the implementation differs from this design, a note in that section says so.

**Update, 2026-11-02 (GL-0093):** the built-in proxy has been removed from the
comm server. `droneengage_udp_proxy` is the only UDP proxy, and it is part of
every server installation. An empty `udp_relay` now means no UDP proxy, and
`udp_relay_fallback` is gone. Anything below about keeping or falling back to
the built-in proxy is history.

Proposal: move the UDP proxy out of `droneengage_server` into its own
process, the **UDP relay server**, the same way mission and task
persistence moved out into `droneengage_storage_server`. The comm server
becomes a broker that asks a relay for a port pair. With no relay
configured it keeps the built-in proxy, so existing deployments don't
change.

This doc is independent of the onboard-server study and of the
mesh/port/plugin plans. Where it touches them (sim topology, port
allocation, plugin events), it names the doc and section.

---

## 0. Why split it

1. **Different load profile.** The comm server routes small JSON
   messages. The proxy moves continuous MAVLink datagrams at telemetry
   rates for every proxied unit, with 4 MB socket buffers each
   (`UDP_RECV/SEND_BUFFER_SIZE`). Scaling, rate limiting and firewall
   rules for the two have nothing in common.
2. **Independent restarts.** Today a comm server restart (memory guard
   `memory_max`, deploy, crash) kills every proxy, so every third-party
   GCS loses its link and the ports change. A separate relay keeps
   forwarding through a comm server restart.
3. **Several instances and placement.** Relays can run on other hosts,
   near the operators, or several of them behind one comm server.
4. **Security.** It's the natural moment to fix the peer-learning
   weakness (§5). Today anyone who can reach a proxy port can send
   MAVLink straight into a flight controller.
5. **Firewall-friendly.** A relay owns a declared port range on one
   host, instead of ephemeral ports spread over the comm server host.

---

## 1. Today's behaviour (verified in the code)

### 1.1 The proxy module — `src/comm_server/js_udp_proxy.js`

| Piece | Behaviour |
|---|---|
| `udp_socket` | One `dgram` udp4 socket, bound `exclusive: true` on `host:port` (port 0 = OS-chosen), 4 MB rcv/snd buffers. On **every** received datagram it overwrites `_caller_ip`/`_caller_port` with the packet's source and calls the forward callback. `sendMessage` sends to that last-learned peer, and drops if none is known yet. `getConfig()` returns `{address: public_host, port}`, or the bound host if it isn't `0.0.0.0` |
| `udp_proxy` | A **pair** of `udp_socket`s. A datagram on socket1 is sent out of socket2 to socket2's last peer, and the other way round. `_blocked` keeps the sockets open but silently drops everything (admin block). Ready callback fires once both sockets have reported |
| `m_activeUdpProxy` | `name → {created, last_access, socket1_port, socket2_port, blocked, m_udpproxy}`. **Keyed by unit name (senderID) only, for the whole process, across accounts** |
| `getUDPSocket(name, s1, s2, cb)` | New name → new pair. Existing name with the same or zero-requested ports → reuse (a pair that isn't ready gets recreated). Different ports → close and recreate |
| `closeUDPSocket(name, cb)` | Close and forget; callback with `en:false` |
| `recreateProxy` | Close and reopen on the **same ports** ("sticky"), keeping the admin block |
| Reaper (`startReaper`, every 60 s, `unref`'d) | (a) a timer gap > 3× the interval means the host was suspended, so every pair is recreated (`recoverAllProxiesAfterResume`); (b) a pair that isn't ready is recreated, and if that fails the unit is notified; (c) a pair idle > 5 min **whose unit is disconnected** is closed and the unit notified. Idle pairs of connected units are kept (telemetry may just be paused). It stops itself when there are no pairs left |
| `notifyUnitProxyClosed` | If the unit's ws is live (`c_activeSenders`), sends it `{mt: 9008, ty: i, tg: name, sd: <comm server>, ms: {en:false, socket1/2: 0.0.0.0:0}}` |
| `fn_getActiveProxiesList` | Snapshot for the authenticator dashboard: name, sockets, ready, blocked, created, last_access |
| `fn_setProxyBlocked` | Sets the in-memory admin block. Lost on restart |
| `checkAndFixKernelBuffers` | At startup (`server.js`), compares `net.core.rmem_max/wmem_max` to 4 MB, tries to raise them (root), else prints sysctl instructions |

### 1.2 The request path — `chat_server/js_chat_system_commands.js:24-71`

- `mt 9008` (`CONST_TYPE_AndruavSystem_UdpProxy`), routing type `s`
  (system), handled by the comm server the unit is connected to.
- **Only actor type `d` (drone) may request.** Others are logged and
  ignored.
- `en:true`: history — on the removed built-in path, if
  `allow_udpproxy_fixed_port !== true`, both requested
  ports are forced to 0. Then `getUDPSocket(p_ws.name, ...)`. The reply
  reuses the request object: `ty: i`, `tg: <unit>`, `sd: <comm server>`,
  `ms: {en, socket1{address, port}, socket2{address, port}}`, sent on the
  same ws. If the pair couldn't be created, the reply is `en:false` with
  ports 0.
- `en:false`: `closeUDPSocket`, then reply `en:false`.
- **Disconnect:** `js_chat_connection.js:233-241`. When a unit's socket
  closes (and wasn't replaced by a newer socket with the same senderID),
  `closeUDPSocket(name)` runs immediately. The reaper's idle path is
  therefore only the fallback for missed closes.

History: the shipped `server.config` used to set
`allow_udpproxy_fixed_port: true`, so in production a unit's requested
socket2 port (its persisted `udp_proxy_fixed_port`, default commented
`15412`) was honoured and proxy ports were **predictable** — which
matters for §5. That key and the built-in proxy are gone; the relay's
`allow_fixed_port` now governs whether requested ports are honoured, and
`peer_change_port` deliberately randomizes free ports (§5.2), so on a
public relay ports are *not* predictable. The sim forces `false` on the
old key anyway.

### 1.3 Admin from the authenticator — `js_andruav_comm_server.js:200-270`

Over the comm server ↔ authenticator S2S link:

| Command | Direction | Effect |
|---|---|---|
| `e` `QUERY_UDP_PROXIES` | auth → comm | reply `f` with `fn_getActiveProxiesList()` (request id echoed) |
| `g` `SET_UDP_PROXY_STATE {name, blocked}` | auth → comm | `fn_setProxyBlocked`, reply `f` with the list plus `{name, blocked, applied}` |
| `f` `REPORT_UDP_PROXIES` | comm → auth | dashboard data |

### 1.4 Who uses it, and which host they assume

- **de_comm doesn't handle 9008.** It passes module system messages
  through to the server.
- **de_mavlink is the only consumer** (also the databus example
  clients). No other module requests a proxy: de_camera only has the
  constant in its copy of `messages.hpp`.
  - Config `udp_proxy_enabled` (`de_mavlink.config.module.json`).
    Persisted `udp_proxy_fixed_port` (local config), settable from the
    webclient through `RemoteCommand_SET_UDPPROXY_CLIENT_PORT` (506,
    needs `PERMISSION_ALLOW_GCS_FULL_CONTROL`).
  - On every (re)connection to the comm server
    (`fcb_main.cpp:1480`) and on a port change (`:2234`) it sends
    `requestUdpProxyTelemetry(en, "0.0.0.0", 0, "0.0.0.0", fixed_port)`.
    It **doesn't retry** after an `en:false` reply until the next
    reconnection or port change.
  - On the 9008 reply (`fcb_andruav_message_parser.cpp:994`, system
    messages only), `updateUDPProxy` opens a UDP client socket to
    **`socket1.address:socket1.port` exactly as received**, so it assumes
    no host of its own.
  - It sends a 1-byte keepalive (`0x00`) only after 15 s without
    receiving anything (`UDP_KEEPALIVE_MS`).
  - It broadcasts `TYPE_AndruavMessage_UDPProxy_Info` (1071)
    `{a: socket2.address, p: socket2.port, en, z: paused, o, e}` to the
    GCSes.
  - **Every MAVLink frame received on the proxy is forwarded to the FC**
    with `sendNative` (`fcb_main.cpp:50`). The only gate is
    `is_gcs_blocked`.
- **The webclient** stores `a`/`p` as
  `m_Telemetry.m_udpProxy_ip/_port`, and shows them in
  `jsc_ctrl_udp_proxy_telemetry.jsx` (plus pause/activate and the port
  dialog, `fn_changeUDPPort`). **The browser never sends UDP itself.**
  The user copies `ip:port` into QGroundControl or Mission Planner, which
  then talks to socket2.

**Consequence for the split:** both de_mavlink and the GCS user take the
host from the reply (`socket*.address` → 1071 `a`). A relay that answers
with **its own `public_host`** works with **unmodified** clients. Only
the security hardening in §5 needs client changes.

---

## 2. The new server

### 2.1 Process

- A Node.js service built on the same base as the storage server
  (`droneengage_server_common` config loader, `$$HASH$$`,
  `server.config.local`, logger). It would live in its own repo, e.g.
  `droneengage_udp_relay_server`, created when phase 1 starts. Not part
  of this design step.
- `src/relay_pairs.js`: `js_udp_proxy.js`'s `udp_socket`/`udp_proxy`/
  reaper/recreate logic moved over, keyed differently (§4) and with
  pinning (§5).
- `src/control_server.js`: wss control API (§2.3) with S2S auth.
- `src/state_store.js`: optional lease persistence, so ports survive a
  relay restart (§2.5).
- `checkAndFixKernelBuffers` moves here. The comm server only needs it
  while it hosts the built-in proxy.

### 2.2 `server.config`, following the storage server's conventions

```jsonc
{
    "server_id"                 : "DE_UdpRelay_1",
    "server_ip"                 : "0.0.0.0",          // control API bind
    "server_port"               : 9100,               // control API (wss)
    "enable_SSL"                : true,
    "ssl_key_file"              : "./ssl_local/ssl_airgap/domain.key",
    "ssl_cert_file"             : "./ssl_local/ssl_airgap/domain.crt",
    "allow_fake_SSL"            : true,
    "ca_cert_path"              : "./ssl_local/ssl_airgap/root.crt",

    // S2S auth for ACCEPTING comm servers (and the authenticator, §2.6),
    // exactly like the storage server:
    //   s2s_cert_enabled=true  -> Ed25519 challenge vs s2s_trusted_server_keys
    //   s2s_cert_enabled=false -> shared-secret HMAC(nonce), secret from file/env
    "s2s_cert_enabled"          : false,
    "s2s_trusted_server_keys"   : { "DE_CommSrv": "./ssl_local/DE_ServerComm_public.pem" },
    "s2s_shared_secret_file"    : "./s2s_shared_secret.key",   // or env de_s2s_shared_secret; min 16 chars; fail closed

    // UDP data plane
    "udp_bind_ip"               : "0.0.0.0",
    "public_host"               : "relay1.example.com",        // returned in every reply
    "udp_port_range"            : [40000, 40999],              // pairs are allocated only here
    "allow_fixed_port"          : true,                        // honour a unit's requested port if inside the range
    "udp_buffer_bytes"          : 4194304,

    // lifecycle (today's constants made configurable)
    "idle_timeout_s"            : 300,
    "reaper_interval_s"         : 60,
    "owner_grace_s"             : 120,                         // §6: comm server link lost -> keep pairs this long
    "state_file"                : "./data/leases.json",        // §2.5, optional

    // security (§5)
    "require_unit_token"        : false,                       // phase 2: true
    "gcs_peer_policy"           : "allowlist_then_tofu",       // open | tofu | allowlist | allowlist_then_tofu
    "max_pps_per_socket"        : 2000,
    "max_bps_per_socket"        : 4000000,

    "ignoreLog"                 : false,
    "log_directory"             : "./logs/",
    "debug_logging"             : false,

    // optional read-only dashboard, same shape as the storage server's
    "dashboard_enable"          : false,
    "dashboard_port"            : 9101,
    "dashboard_listening_ip"    : "127.0.0.1",
    "admin_username"            : "admin",
    "admin_password"            : "CHANGE_ME",
    "session_secret"            : "change-this-secret-in-production"
}
```

### 2.3 Control API (wss, after S2S auth)

The envelope is the storage server's `{mt, ms, rid}`. Replies echo
`rid` and carry `success`/`error`. Push events have no `rid`.

| Request | `ms` fields | Reply |
|---|---|---|
| `open` | `account, group, unit, req{p1, p2}` (0 = any), `cap[]` (client capabilities, e.g. `"tk"`), `gcs_ips[]` (optional allowlist seed) | `{en, socket1{address, port}, socket2{address, port}, tk?, relay_id, pair_id}` |
| `close` | `account, unit` | `{en:false}` |
| `unit_offline` | `account, unit` | starts the idle clock now (§6) |
| `unit_online` | `account, unit` | cancels a pending close |
| `sync` | `units[{account, unit}]`, sent on (re)connect: the owner's full list of online units | `{kept[], dropped[]}` |
| `block` | `account, unit, blocked` | `{applied}` |
| `peer_allow` | `account, unit, ips[]` | `{applied}` (§5.2) |
| `peer_reset` | `account, unit, side: gcs\|unit` | unpins that side |
| `list` | `account?` | today's `fn_getActiveProxiesList` shape plus `account, owner, pinned{unit, gcs}, rx/tx counters` |
| `stats` | — | `{pairs, ports_free, pps, bps, rejected_by_reason{...}}` |

| Push | Payload | Purpose |
|---|---|---|
| `closed` | `account, unit, reason` | the relay closed a pair (idle, recreate failed, admin). The owning comm server tells the unit (today's `notifyUnitProxyClosed`) |
| `changed` | `account, unit, socket1, socket2` | ports changed (e.g. restart without a state file). The owner sends the unit an unsolicited 9008 `en:true`, which de_mavlink already handles |

### 2.4 Ownership

Each pair records the `server_id` of the comm server that opened it (its
**owner**). Only the owner can `close`, `unit_offline`, `peer_allow` or
`peer_reset` it. `block`/`list` are also allowed for the authenticator
role (§2.6). Several comm servers can share one relay without seeing or
closing each other's pairs.

### 2.5 Port allocation and stickiness

- Pairs come only from `udp_port_range`. Allocation is lowest free
  even/odd neighbours, for readable dashboards; correctness doesn't
  depend on it. `req.p2` (a unit's fixed client port) is honoured only if
  `allow_fixed_port` is set and the port is inside the range and free.
  Otherwise the reply has a different port, as it does today with fixed
  ports off.
- With `state_file`, leases `{account, unit, p1, p2, owner, blocked}`
  are written on change. After a relay restart the same ports are
  re-bound, so a QGC user who typed `ip:port` keeps working.

### 2.6 Several instances, and the authenticator

- Every instance has its own `server_id`, `public_host` and range.
- The authenticator's dashboard needs the proxy list. Two options:
  1. **through the comm servers** (the current path, `e`/`f`/`g`):
     the comm server answers from `list` on its relays. No authenticator
     change; phase 1.
  2. the authenticator connects to relays directly as an S2S peer
     (trusted key on the relay) for list/block. Phase 3, and only if
     operators run relays without a comm server between them and the
     dashboard.

---

## 3. The comm server side

### 3.1 `server_to_server/js_udp_relay_client.js`

Modelled on `js_db_proxy_client.js`, but **one connection per configured
relay** (the storage client is a singleton):

- wss with `rejectUnauthorized: false`, as the storage link does. It
  answers the challenge with Ed25519 (`s2s_my_private_key`/`server_id`)
  or HMAC, depending on the challenge `method`. It **fails closed** if
  no challenge or `auth_success` arrives within
  `CONST_S2S_AUTH_HANDSHAKE_TIMEOUT`.
- Exponential backoff 1 s → 60 s with ±25 % jitter. On every
  successful auth it sends `sync` with the units this comm server
  currently holds pairs for.
- Request timeout of 10 s per `rid`, with pending requests rejected on
  close.
- A 30 s heartbeat reporting status to the authenticator: a new command
  `j` `UDP_RELAY_STATUS {relay_id, host, port, status, pairs, error}`,
  the same shape as `d` `STORAGE_STATUS`.

### 3.2 Config on the comm server

```jsonc
"udp_relay": [                                  // absent or [] -> built-in proxy (today)
    { "host": "127.0.0.1", "port": 9100, "id": "DE_UdpRelay_1" }
],
"udp_relay_select": "hash",                     // hash | first_healthy | least_loaded  (§7)
"udp_relay_fallback": "none",                   // none | builtin   (when all relays are down)
"udp_relay_pending_s": 30                       // hold an open request while relays reconnect
```

### 3.3 The `UdpProxy` case

```
9008 from a drone (actor 'd', as today)
 ├─ no udp_relay configured  -> today's js_udp_proxy path, byte-for-byte unchanged
 └─ udp_relay configured
     ├─ en:true  -> relay = select(account, unit); relay.open(...)
     │              reply to the unit with the relay's sockets (same 9008 shape)
     │              relay unreachable: hold up to udp_relay_pending_s, then
     │                 fallback=builtin -> today's path
     │                 fallback=none    -> en:false (as today on failure)
     │              and remember "unit wants a proxy", so a later relay
     │              recovery re-opens it and pushes 9008 to the unit
     │              (de_mavlink doesn't retry by itself, §1.4)
     └─ en:false -> relay.close(...)
```

- The disconnect hook (`js_chat_connection.js:240`) calls
  `relay.unit_offline(account, unit)` instead of `closeUDPSocket`. It
  doesn't close immediately (§6).
- `e`/`g` from the authenticator are answered from local pairs plus
  relay `list`, and `g` maps to relay `block`.
- `closed`/`changed` pushes from a relay are turned into the 9008
  messages the unit understands.
- **Default `udp_relay_fallback: none`.** An operator who configured a
  relay chose its public host, port range and firewall rules. Silently
  serving from the comm server's host would hand out an endpoint the
  firewall may block, and would bypass the relay's pinning. `builtin` is
  available for operators who prefer availability.

---

## 4. Binding to auth sessions and accounts

1. **Only units with live sessions.** The comm server already enforces
   this: 9008 is only handled for an accepted ws with actor `d`. The
   relay trusts its S2S-authenticated comm servers for that, and doesn't
   call the authenticator per request.
2. **Account scoping.** Pairs are keyed by `(account, unit)`, not by name
   alone. Today `m_activeUdpProxy[name]` is global, so a unit `drone1`
   in account A and `drone1` in account B **share one pair**: B's
   request reuses A's sockets and gets A's ports. The relay fixes this
   by construction. (The built-in path keeps the current behaviour.
   Fixing it there too is a one-line key change, listed as a follow-up
   in §9.)
3. **Direct requests with a login token.** Not in v1. de_mavlink would
   need a wss client and a credential, the comm server already proves the
   session, and nothing today needs it. **Room is left for it:** `open`
   carries `account/group/unit` explicitly, and the relay authorises the
   *caller* (S2S peer), not the unit. A later
   `open {…, session_token}` from a non-comm-server peer (an onboard
   server, a future direct client) is checked against the authenticator's
   signing key and needs no change to the pair model.

---

## 5. Security: peer pinning

### 5.1 The problem today

Each `udp_socket` re-learns its peer from **every** datagram it receives
(`_caller_ip/_caller_port` are overwritten in the `'message'` handler),
and nothing checks who sent it. The ports are predictable in production
(fixed ports, §1.2), or easy to find by scanning.

- **Command injection, the serious one:** a datagram sent to **socket2**
  is forwarded to the drone, and de_mavlink passes every MAVLink frame
  to the flight controller (`sendNative`, §1.4). Anyone who can reach
  socket2 can arm, change mode or upload missions. Every packet the
  attacker sends also makes them the socket2 peer, so they receive the
  vehicle's telemetry until the real GCS sends again.
- **Hijack of the drone side:** a datagram to socket1 makes the attacker
  socket1's peer. GCS → drone traffic then goes to the attacker until
  the drone sends again (it does continuously while telemetry flows,
  but only every 15 s when idle).

### 5.2 Design

**Unit side (socket1): per-pair token.** This side is our own code
(de_mavlink), so it can carry a secret.

- `open` returns `tk`: 16 random bytes, base64url, in the 9008 reply,
  only if the request advertised `cap: ["tk"]`.
- de_mavlink sends a **hello** datagram, `"DEU1" | tk (16 bytes)`, when
  the socket opens and **every 5 s**. This replaces the 1-byte idle
  keepalive for capable clients. Hello datagrams are never forwarded.
- The relay **pins** socket1's peer to the source of the latest valid
  hello. Datagrams from any other source are dropped (counted as
  `rejected.unit_unpinned`). A NAT rebinding is re-pinned by the next
  hello, within 5 s.
- **Compatibility:** a request without `cap: ["tk"]` (today's
  de_mavlink) gets no token and today's last-sender behaviour, unless
  `require_unit_token: true`, in which case it's refused with
  `en:false, err: "token_required"`.
- MAVLink frames start with `0xFD`/`0xFE`, so `"DEU1"` (0x44…) can't
  collide with them.

**GCS side (socket2): allowlist plus trust-on-first-use (TOFU).** This
side is a third-party GCS (QGC, Mission Planner) that can't carry a
DroneEngage token.

- **Allowlist from the authenticated GCS sessions.** The comm server
  knows the remote IP of every GCS websocket in the unit's account and
  group (behind a reverse proxy: `X-Forwarded-For` via a new
  `trust_proxy_header` setting). It seeds `open.gcs_ips` with them, and
  sends `peer_allow` when a GCS joins that group or presses **Activate**
  in `jsc_ctrl_udp_proxy_telemetry`. A desktop GCS usually sits behind
  the same NAT as the browser, so the IPs match. Datagrams to socket2
  from IPs outside the list are dropped.
- **TOFU fallback** (`allowlist_then_tofu`): when the allowlist is empty,
  for example an operator using QGC alone on a network where no browser
  session exists, the first source pins socket2 and later sources are
  dropped until a GCS presses **Reset peer** (`peer_reset`). This is
  already much stronger than "last sender wins".
- `open` keeps today's behaviour, for closed networks only.
- **Optional end-to-end:** MAVLink 2 signing on the proxy channel. QGC
  and Mission Planner support a signing key. de_mavlink would accept
  only signed frames from the proxy (config `udp_proxy_require_signing`).
  This protects the FC even if the relay, or the path to it, is
  compromised. It's opt-in because it needs key setup on both ends.
- **Rate limits** per socket (`max_pps`/`max_bps`) and oversized
  datagrams dropped, so one pair can't flood the FC link or the relay.

### 5.3 Client changes this needs

> **Implementation note (TASK-54).** The optional `pin` field on 1071, listed
> in the webclient row below, was **not** implemented. de_mavlink sends 1071
> and can't know the relay's pin state. Instead, a GCS uses the comm-server
> system message **9021 `UdpProxyPeer`** `{tg_unit, act}` where `act` is
> `allow`, `reset` or `query`, plus the takeover actions: `switch` `{ip,
> port}` approves the waiting request and moves the lock to that address
> (`peer_switch`); `reject` `{ip, secs?}` drops that IP for `secs` seconds
> (`peer_reject`, default `gcs_reject_default_s`); `kick` `{secs?}` drops
> the current GCS IP and frees the pin (`peer_kick`); `change_port` moves
> socket2 to a fresh port (`peer_change_port`). Client-sent `secs` is
> capped at 3600 s server-side, then at the relay's `gcs_reject_max_s`.
> The reply is `{tg_unit, pinned, applied?, secs?}` or `{tg_unit, error}`
> — `secs` echoes the effective reject duration for `reject`/`kick`. The
> GCS must be in the unit's account and group, and everything except
> `query` needs `PERMISSION_ALLOW_GCS_FULL_CONTROL`. Without a relay pair
> 9021 answers `pinned: {gcs: {mode: 'open'}}`; with a disconnected relay
> it answers `error: relay_unavailable`. **Activate** maps to `allow` and
> **Reset peer** to `reset`.
>
> The comm server also *pushes* 9021 to every GCS socket in the unit's
> account+group holding `PERMISSION_ALLOW_GCS_FULL_CONTROL`:
> `{tg_unit, act: 'attempt', from_ip, from_port, others, until_ms}` when a
> stranger knocks on a locked tofu pair (`until_ms` is when the request
> expires — the badge counts down), `{tg_unit, act: 'switched', from_ip,
> from_port, reason}` when the pin moves (`reason` is `same_ip`, `idle`,
> `approved` or `kicked`), `{tg_unit, act: 'changed', socket1, socket2}`
> when the pair's sockets move (e.g. after `change_port`), and `{tg_unit,
> act: 'state', pinned}` after a mutating act lands, so an answered
> request clears everywhere.

| Component | Change |
|---|---|
| de_mavlink | advertise `cap: ["tk"]` in 9008; send the `DEU1` hello at open and every 5 s; keep everything else (it already re-inits on an unsolicited 9008 `en:true`); optional `udp_proxy_require_signing` |
| de_comm | none (it passes 9008 through) |
| webclient | show pin state and relay host (1071 gains optional `pin: {gcs: "allowlist"\|"tofu"\|"open", ok}`); **Activate** also authorises this browser's IP; new **Reset peer** button; signing-key hint in the dialog |
| comm server | `js_udp_relay_client.js`, the §3.3 routing, GCS-IP allowlist feed, `trust_proxy_header` |
| authenticator | dashboard: relay instances (`j` status), per-pair account, owner and pin state |
| databus examples | optional: the hello, if they're kept as proxy clients |

---

## 6. Unit presence and lifecycle outside the comm server

The relay can't see websockets, so presence comes from the owner:

- **`unit_offline`** from the comm server's disconnect hook (the core
  path, §3.3). The relay doesn't close right away: it starts the idle
  clock and closes after `idle_timeout_s` with no traffic, which is
  today's "idle + disconnected" reaper rule. This also keeps a unit's
  ports through a quick de_comm reconnect, where today the pair is
  closed and reopened.
- **`unit_online` / a new `open`** cancels that.
- **Owner link lost** (comm server restart): the relay keeps that
  owner's pairs for `owner_grace_s`, still forwarding. When the owner
  reconnects it sends `sync`, which keeps the pairs of units that are
  still online and lets the rest age out. If the owner doesn't come back
  within the grace period, its pairs are closed. This is the restart
  resilience from §0.2.
- **Health and resume:** the reaper's not-ready recreate and its
  suspend/resume detection move over unchanged. A failed recreate becomes
  a `closed` push.
- **Plugin events** (`PluginEvents.md` `client.close`): **not** used for
  lifecycle. Plugins there are optional and observe-only (§3 of that
  doc), and pair cleanup must not depend on whether an operator loaded a
  plugin. The plugin events are still the right way to observe this
  (§8.2).

---

## 7. Choosing an instance

| Strategy | How | When |
|---|---|---|
| `hash` (default) | rendezvous hash of `(account, unit)` over the healthy relays | stable: a unit lands on the same relay every time, so its ports persist; failover moves only the units of the relay that failed |
| `first_healthy` | the first listed relay that's up | primary/standby setups |
| `least_loaded` | the lowest `stats.pairs` | big fleets of short sessions |
| authenticator-assigned | the authenticator picks a relay per login, sent to the comm server with the login request | later (phase 3), when relays are shared across comm servers and placement policy lives centrally |
| nearest | relay closest to the GCS operator | out of scope: needs GCS location and a geo model, and the GCS isn't known when the unit requests |

---

## 8. Migration, simulator, phases

### 8.1 Compatibility guarantees

- **No `udp_relay` key, no change.** The built-in `js_udp_proxy.js` and
  every wire message stay as they are.
- Wire formats are only **extended**: 9008 gains optional `cap` (request)
  and `tk` (reply), and the new 9021 carries the pin state (in place of
  the 1071 `pin` field first proposed here, see §5.3). Old de_mavlink
  versions and old webclients ignore unknown fields.
- `allow_udpproxy_fixed_port` keeps its meaning only as history for the
  removed built-in path. The relay's `allow_fixed_port` plus the range
  replaces it there.
- Security defaults during migration: `require_unit_token: false`,
  `gcs_peer_policy: allowlist_then_tofu`. TOFU and the allowlist need no
  client change, so **operators get injection protection on day one**
  just by switching to a relay. The token side tightens once de_mavlink
  is updated.

### 8.2 Simulator

Using `droneengage_studio/docs/real_servers_mesh.md` and
`docs/port_allocation.md`:

- **A new node type** in the `servers:` topology:

  ```yaml
  servers:
    hq:     {type: comm, gcs: true, udp_relay: [relay1]}
    relay1: {type: udp_relay, gcs_peer_policy: allowlist_then_tofu}
  ```

  `_gen_udp_relay(node)` writes its `server.config` like `_gen_storage`:
  per-run shared secret, `public_host: 127.0.0.1`, a log dir. The repo
  is resolved via a new `DE_UDP_RELAY_DIR` in `sim.local.env`.
- **Ports** through the allocator: `udp_relay.<node>.wss` (control) and
  an `alloc_block("udp_relay.<node>.range", size=2*N)` for the data
  range, with N = the number of units with `udp_proxy_enabled`. The
  block's bounds go into `udp_port_range`. Launch order: after storage,
  before comm servers.
- **Faults:** a `link_proxy` on the named connection `hq->relay1`
  (control plane) tests owner grace and `sync`. Data-plane loss stays
  with the existing per-unit tools.
- **Assertions:**
  - a new check `udp_proxy_mavlink {unit, secs, at_least}` where the
    runner (pymavlink `udpout`) dials the 1071 `a:p` and counts
    HEARTBEATs
  - `udp_proxy_rejects {unit, injected_from: other_ip}`, for which the
    sim sends from a second loopback address (127.0.0.2) and expects 0
    frames to reach SITL. FC-side checks (`mission_items`,
    `telemetry_value`) prove nothing was injected.
  - comm-side plugin events once they exist (`client.close`), plus relay
    `stats` through `./sim ctl`.
- Example scenarios: relay restart with a `state_file` (same ports
  after), comm server restart within `owner_grace_s` (telemetry keeps
  flowing), and cross-account same-name units getting distinct pairs.

### 8.3 Phases

| Phase | Delivers | Acceptance criteria |
|---|---|---|
| **P1 Relay server, compatibility mode** | new repo; pair engine moved over, keyed by `(account, unit)`; control API (`open/close/unit_offline/unit_online/sync/block/list/stats`); `js_udp_relay_client.js`; §3.3 routing with `fallback`; `e/g/f` served from the relay; `j` status; port range; `public_host` | With no `udp_relay`: comm server unit tests and a sim run unchanged. With a relay: QGC (or pymavlink in the sim) gets telemetry via the 1071 `a:p` on the relay host; a comm server restart within the grace keeps the flow alive; two accounts with unit `drone1` get different pairs; admin block from the authenticator dashboard drops packets |
| **P2 GCS-side pinning** | `allowlist`/`tofu` policies, `peer_allow`/`peer_reset`, GCS-IP feed from the comm server, `trust_proxy_header`, rate limits | a datagram to socket2 from a non-allowlisted source never reaches SITL (sim check); after **Reset peer** a new source can pin; webclient shows the pin state |
| **P3 Unit token** | `cap:["tk"]`, `tk` in replies, `DEU1` hello in de_mavlink, `require_unit_token` | a forged socket1 source gets nothing forwarded; a NAT rebinding is re-pinned within 5 s; old de_mavlink still works with `require_unit_token: false` and is refused with `true` |
| **P4 Durability and placement** | `state_file`, `changed` push and unsolicited 9008, `hash`/`least_loaded` selection over several relays, optional authenticator-assigned relays | relay restart: same ports, QGC reconnects without retyping; killing one of two relays moves only its units, and they get new ports pushed |
| **P5 Optional MAVLink 2 signing** | `udp_proxy_require_signing` in de_mavlink, webclient key hint | unsigned frames from the proxy are dropped by de_mavlink; a QGC configured with the key controls the vehicle |

Phase 1 is useful on its own (restart resilience, account scoping,
firewall-friendly ranges). Phase 2 is the most urgent security win and
needs no drone-side change.

---

## 9. Open questions and follow-ups

1. **UDP for units that re-parent over Wi-Fi** (out of scope). A unit
   that joins a neighbour has no direct internet route to the relay. The
   design leaves room in two ways:
   - **Pinning is by token, not by fixed IP**, so packets can arrive
     from a different source after re-routing, for example through the
     neighbour's NAT or a forwarder on the neighbour, and the next hello
     re-pins. The pair and its public ports don't change, so the QGC
     user notices nothing.
   - **`open` authorises the caller, not the unit's address** (§4.3), so
     a neighbour's onboard server could open or refresh pairs on a
     joined unit's behalf.

   What's missing is a UDP forwarder on the neighbour, or a relay
   instance running on it. That decision belongs to the re-parenting
   study.
2. **Built-in path account-key bug.** Keying `m_activeUdpProxy` by
   `(account, name)` in the current comm server would fix cross-account
   collisions for deployments that never adopt the relay. It's a small,
   separate fix. Should it be done now, independent of this design?
3. **Default fallback.** `none` (this doc) or `builtin` when every relay
   is down?
4. **The allowlist for mobile and CGNAT GCSes:** the browser and QGC may
   leave from different public IPs (a phone tethering a laptop, for
   example). Is TOFU plus Reset an acceptable fallback, or should the
   webclient also offer "allow any IP for N minutes"?
5. **Hello interval:** 5 s costs about 20 B/s per unit. Fine on
   cellular, but configurable?
6. **Should the authenticator talk to relays directly** (§2.6 option 2),
   or always go through comm servers?
7. **Repo name and ownership:** a new `droneengage_udp_relay_server`, or
   a second entry point inside `droneengage_storage_server`'s family of
   services?
8. **IPv6:** today's sockets are `udp4` only. Should a relay offer v6
   pairs (`public_host_v6`)?
