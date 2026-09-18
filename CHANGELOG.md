# Changelog

Major changes to the DroneEngage main signaling (communication) server, newest first.

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
