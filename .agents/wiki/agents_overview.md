# droneengage_server — overview

Andruav/DroneEngage communication (signaling) server — WebSocket
messaging between units and GCSs, parent/child server-to-server mesh
relay, message routing (group + individual targeting), system commands,
and storage-server integration via DBProxyClient. **Rust (tokio)** port
of the Node.js server; wire API, `server.config` and state formats are
unchanged. Node code on `pr_nodejs_last_version`. See `../../AGENTS.md`
for the workspace and shared server conventions.

## Build / test / run

    cargo build --release     # -> target/release/andruavmainserver (no plugins)
    cargo test                # unit + integration tests (tests/)
    cargo test --test relay   # one integration test file

The comm server actually deployed is built by the
`../droneengage_server_plugins` workspace (same binary name, plugins
compiled in) and **run from this directory** so it loads this repo's
`server.config`, `ssl_local/`, `data/`. `run.home.sh` / `run.drone.sh` /
`run.server.sh` — run scripts; `../sh_start_servers.sh` — all servers.

## Config

`server.config` + `server.config.local`. `command_plugin` entries enable
plugins — matched by path components, so the JS-era `./plugins/...`
paths still work.

## Source layout

`src/` — `main.rs`, `cli.rs`, `lib.rs`, `server.rs`, `hub.rs`,
`comm_server/` (client sessions, routing, system commands), `s2s/`
(server mesh + Ed25519/HMAC handshake), `chat/`, `plugins/` (plugin host
interface), `udp_proxy.rs`, `config.rs`, `constants.rs`. `tests/` —
relay, routing, tasks, e2e, udp_proxy integration tests. `wiki/` —
design docs (e.g. `UdpRelayServer.md`). `deployment/` — deploy configs.
