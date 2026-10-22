# Changelog

Major changes to the DroneEngage main signaling (communication) server, newest first.

## [Unreleased]

- **UDP relay server support.** With `udp_relay` configured, 9008 requests
  are brokered to one or more standalone relay servers over an
  S2S-authenticated wss link, instead of the built-in UDP proxy:
  - Relays are chosen per unit with `udp_relay_select` (`hash`,
    `first_healthy` or `least_loaded`).
  - Units fail over after `udp_relay_failover_s`.
  - `udp_relay_fallback` (`none` or `builtin`) and `udp_relay_pending_s`
    apply while no relay is reachable.
  - Without `udp_relay`, nothing changes.
- New system message **9021 `UdpProxyPeer`**: a GCS can allow its IP, reset
  or query a relay pair's GCS-side pin.
- GCS IPs are fed to the relay's allowlist. The new `trust_proxy_header`
  takes the IP from `X-Forwarded-For` behind a reverse proxy.
- The AUTH dashboard lists relay pairs with their account, owner and relay
  (`e`). Block (`g`) works on relay pairs. A new `j UDP_RELAY_STATUS`
  heartbeat reports each relay link.

## [5.6.2] - 2026-09-15

- Added UDP proxy block/resume control driven from the AUTH dashboard.
- The server now reports each unit's login requestId in account details.

## [5.6.1] - 2026-09-09

- Added AIS stream message constants and plugin configuration.
- Added a `debug_logging` config flag to reduce routine log spam.

## [5.6.0] - 2026-09-05

- Added the UDP proxy dashboard query used by the authenticator admin view.
- Improved disconnect cleanup for proxied units.

## [5.5.x] - 2026-09-02

- UDP proxies are now cleaned up when a vehicle disconnects; paused-but-connected units are no longer reaped.
- Added the News feature: the comm server relays news items and pushes them to connected clients.
- Shared config loading and helpers moved to the `droneengage_server_common` npm package.

## [5.4.x] - 2026-08

- WebSocket authentication moved from URL query-string credentials to a post-handshake auth frame (more secure); a compatibility flag keeps old clients working.
- Added the server plugins folder.

## [5.3.0] - 2026-08-09

- Source code restructured under `src/` (`server/` renamed to `comm_server/`).
- Startup info now shows the public host name.
- Added a build script that produces a clean distribution copy.

## [5.2.0] - 2026-08-08

- Added Storage Server integration: mission load/save/delete for GCS clients.
- Added the QueryServer/StateServer system that reports storage connection status.
- Server port can be overridden with the `de_comm_server_port` environment variable.

## [5.x and earlier] - 2026-07 and earlier

- Added the UDP proxy feature (including fixed-port mode for air-gapped servers).
- Added login permission checks on messages and binary payloads.
- Added the s2s (server-to-server) super/local server link.
- Added log files, memory/heap diagnostics, and routing performance fixes.
- Early releases: WebSocket agent/GCS routing, group broadcasts, and authentication against the auth server.
