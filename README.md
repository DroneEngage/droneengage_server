[![Ask DeepWiki](https://deepwiki.com/badge.svg)](https://deepwiki.com/DroneEngage/droneengage_server)

# Andruav Server

Andruav Server is the communication server responsible for exchanging messages between different units and GCSs running [Andruav or Drone-Engage](https://cloud.ardupilot.org).

## Architecture Overview

The communication server provides:
- WebSocket-based real-time messaging between units and GCS
- Server-to-server mesh relay for scalable message propagation
- Message routing with group and individual targeting
- System commands and task management
- Server-to-Server (S2S) authentication for secure relay connections
- Frame-based client WebSocket authentication (`de_auth` frame) with optional legacy query-string compatibility
- Storage server integration via DBProxyClient for task persistence

### Server Roles

The server can operate in different modes:
- **Standalone**: Independent server for local communication
- **Child Server**: Connects to a parent server for relay
- **Parent (Super) Server**: Accepts child connections and forwards messages

## Installation

### Prerequisites

- Rust (stable, via [rustup](https://rustup.rs))
- OpenSSL (for SSL certificates)

### Setup

```bash
# Clone the repository
git clone https://github.com/DroneEngage/droneengage_server.git
cd droneengage_server

# Build
cargo build --release

# Copy configuration
cp server.config server.config.local
# Edit server.config.local with your settings

# Generate SSL certificates (if needed)
mkdir -p ssl_local/ssl_airgap
openssl req -x509 -newkey rsa:4096 -keyout ssl_local/ssl_airgap/domain.key -out ssl_local/ssl_airgap/domain.crt -days 365 -nodes
```

## Configuration

The server is configured via `server.config` (JSON format). Key settings:

### Server Settings

```json
{
    "server_id": "DE_CommSrv",
    "server_ip": "::",
    "public_host": "127.0.0.1",
    "server_port": 9966,
    "s2s_ws_target_ip": "127.0.0.1",
    "s2s_ws_target_port": "19001"
}
```

### Server Roles

**Parent (Super) Server Mode:**
```json
{
    "enable_super_server": true,
    "s2s_super_server_ip": "127.0.0.1",
    "s2s_super_server_port": 9866
}
```

**Child Server Mode:**
```json
{
    "enable_persistant_relay": true,
    "s2s_relay_to_super_server_ip": "127.0.0.1",
    "s2s_relay_to_super_server_port": 9866
}
```

### S2S Authentication

Server-to-Server authentication using Ed25519 keys. The comm server signs challenges with its private key:

```json
{
    "s2s_my_private_key": "./ssl_local/DE_ServerComm_private.pem"
}
```

**Note:** Comm servers always attempt authentication when challenged. The accepting server (Auth Server or parent server) controls whether authentication is required via its `s2s_auth_enabled` configuration.

See [wiki/S2SAuthentication.md](wiki/S2SAuthentication.md) for detailed setup.

### Client WebSocket Authentication

Clients authenticate to the comm server's WebSocket endpoint using a `de_auth` system frame instead of passing credentials in the URL query string. Frame-based auth is **always enabled**.

**Protocol:**
1. Client opens the WebSocket **without** credentials in the query string.
2. Client sends a `de_auth` system frame as the **first** WS message:
   ```json
   { "ty": "s", "mt": "de_auth", "f": "<temp_login_key>", "s": "<sender_id>", "at": "<actor_type>" }
   ```
   - `f` — temp login key (`CONST_CS_LOGIN_TEMP_KEY`)
   - `s` — party/sender ID
   - `at` — actor type
3. Server validates the frame and replies with a `de_auth_ack`:
   ```json
   { "ty": "s", "mt": "de_auth_ack", "r": "ok" }
   ```
   On failure `r` is `"fail"` with an `em` error message, and the socket is closed.
4. If no auth frame arrives within **8 seconds**, the socket is closed with `r: "fail"`, `em: "auth frame timeout"`.

**Legacy compatibility flag:**

```json
{
    "ws_auth_no_frame_old_compatibility": true
}
```

| Value | Behavior |
|-------|----------|
| `true` (default) | Legacy clients that still send credentials in the URL query string are accepted (with a warning), alongside the frame path. |
| `false` | Query-string credentials are **rejected**; only the `de_auth` frame path is allowed. Recommended for new deployments. |

### SSL/TLS

```json
{
    "ssl_key_file": "./ssl_local/ssl_airgap/domain.key",
    "ssl_cert_file": "./ssl_local/ssl_airgap/domain.crt",
    "allow_fake_SSL": true,
    "ca_cert_path": "./ssl_local/ssl_airgap/root.crt"
}
```

### Database

```json
{
    "dbIP": "localhost",
    "dbuser": "USERNAME",
    "dbpassword": "PASSWORD",
    "dbdatabase": "andruav"
}
```

### Storage Server Integration

The comm server connects directly to a storage server for task persistence:

```json
{
    "enable_storage_server": true,
    "storage_server_host": "127.0.0.1",
    "storage_server_port": 9000
}
```

The DBProxyClient handles:
- Direct WebSocket connection to storage server (always wss://)
- S2S authentication using Ed25519 keys (independent of SSL)
- Exponential backoff reconnection on disconnect
- Periodic heartbeat (30s) with connection status reporting to AUTH
- Task operations (LoadTasks, SaveTasks, DeleteTasks, DisableTasks)

SSL is independent of S2S certificate authentication - similar to the AUTH <--> CommServer pattern. The storage server has its own SSL configuration.

### UDP Relay Server

The UDP proxy is the 9008 port pair that relays MAVLink between a drone's
de_mavlink and a third-party GCS such as QGroundControl or Mission Planner. It
runs only in the standalone server (`droneengage_udp_proxy`; design in
[wiki/UdpRelayServer.md](wiki/UdpRelayServer.md)), which is part of every server
installation. The comm server is a broker: it asks a relay for the pair and
replies to the unit with the relay's sockets in the 9008 shape. With `udp_relay`
empty, the server warns at startup and drones get `en:false`.

| Key | Default | Meaning |
|---|---|---|
| `udp_relay` | shipped: the local relay | relays as `[{ "host", "port", "id" }]`; absent or `[]` = no UDP proxy |
| `udp_relay_select` | `"hash"` | which relay a unit goes to: `hash` (stable per account/unit), `first_healthy` (list order) or `least_loaded` (fewest pairs) |
| `udp_relay_pending_s` | `30` | how long a request is held while relays reconnect; then the unit gets `en:false`, and an unsolicited `en:true` once a relay is back |
| `udp_relay_failover_s` | `120` | a relay down this long has its units moved to another relay; match the relays' `owner_grace_s` |
| `trust_proxy_header` | `false` | `true` only behind a reverse proxy: a party's IP (fed to the relay's GCS allowlist) is then the first `X-Forwarded-For` entry |

```json
"udp_relay": [{ "host": "127.0.0.1", "port": 9100, "id": "DE_UdpRelay_1" }]
```

The link to the relay uses the same S2S handshake as the storage server. If
the relay runs `s2s_cert_enabled: false`, it uses the shared secret from
`s2s_shared_secret_file` / `de_s2s_shared_secret`, which must equal the
relay's secret. Otherwise it uses Ed25519 with `s2s_my_private_key`, and the
relay lists this `server_id` in its `s2s_trusted_server_keys`. Self-signed
relay certificates are accepted, as on the storage link.

When the relay and the comm server are installed together, copy the relay's
`s2s_shared_secret.key` next to this server's config (the shipped
`s2s_shared_secret_file` points there), or export `de_s2s_shared_secret` —
the secret is not created here, and without it the relay link never
authenticates and every drone's 9008 answers `en:false`. A missing secret
prints a startup warning naming both the file and the variable.

What the broker does:
- **Unit disconnect:** the relay is told `unit_offline`, and the pair idles
  out there. A quick reconnect keeps the same ports.
- **Relay pushes:** `closed` becomes 9008 `en:false` to the unit. `changed`
  (for example after a relay restart without free ports) becomes an
  unsolicited 9008 `en:true`.
- **GCS pinning:** relay pairs only accept GCSes from the unit's
  account+group (their IPs are sent to the relay). A GCS manages the pin with
  system message **9021** (`UdpProxyPeer`) `{tg_unit, act: allow|reset|query}`.
  `allow` and `reset` need `PERMISSION_ALLOW_GCS_FULL_CONTROL`.
- **AUTH dashboard:** `e` lists relay pairs, with
  `account`, `owner` and `relay_id`. `g` blocks on the relay. A new `j`
  `UDP_RELAY_STATUS` reports each relay link on connect, on change and
  every 30 s.

## Running the Server

### Development

```bash
cargo run -- --config=server.config
```

Paths in the configuration (SSL files, keys, plugins) resolve from the
working directory, so run the server from the repository root. The binary
also takes `--help` and `--version`.

### Production

```bash
# Using deployment scripts
./deployment/run_parent.sh    # For parent server
./deployment/run_slave.sh     # For child server

# Or directly
./target/release/andruavmainserver --config=server.config
```

## Message Propagation

The server implements a mesh relay system for message propagation:

- **Local messages**: From WebSocket clients, forwarded to relay servers
- **External messages**: From relay servers, delivered locally only (no re-forwarding)
- **Loop prevention**: Uses `_path` array to track message traversal

See [wiki/MessagePropagation.md](wiki/MessagePropagation.md) for detailed architecture.

## Message Routing

Messages are routed based on the `ty` (type) and `tg` (target) fields:

### Routing Types

| Type | Constant | Behavior |
|------|----------|----------|
| `'g'` | `CONST_WS_MSG_ROUTING_GROUP` | Broadcast to group |
| `'i'` | `CONST_WS_MSG_ROUTING_INDIVIDUAL` | Targeted delivery |
| `'s'` | `CONST_WS_MSG_ROUTING_SYSTEM` | System commands (local only) |

### Target Values

| Value | Constant | Meaning |
|-------|----------|---------|
| `'_GCS_'` | `CONST_WS_SENDER_ALL_GCS` | All GCS units |
| `'_GD_'` | `CONST_WS_SENDER_ALL` | All units (GCS + Agents) |
| `'_AGN_'` | `CONST_WS_SENDER_ALL_AGENTS` | All drone agents |
| *(other)* | - | Specific unit ID (one-to-one) |

## Testing

```bash
# Run all tests
cargo test

# Run one test file
cargo test --test relay
```

Tests are in `tests/`. CI also runs `cargo fmt --check` and
`cargo clippy --all-targets -- -D warnings`. `tests/udp_relay.rs` runs
against a real relay binary when `DE_UDP_RELAY_BIN` points to one (or a
sibling `droneengage_udp_proxy` checkout has been built) and is skipped
otherwise.

## Plugins

Command plugins are Rust crates compiled into the binary, not files loaded at
run time. `command_plugin` entries select registered plugins by a component
of their path, so `"./plugins/command_processing/js_andruav_command_processing"`
selects `command_processing`. This repository builds the server without
plugins. [droneengage_server_plugins](https://github.com/DroneEngage/droneengage_server_plugins)
(branch `pr_rust`) builds the same server with its plugins registered.

## Shared code

Configuration loading, JavaScript-compatible JSON handling, the file
logger, TLS/WebSocket plumbing and S2S authentication come from
[droneengage_server_common](https://github.com/DroneEngage/droneengage_server_common)
(branch `pr_rust`), shared with the storage server and the authenticator.

## Differences from the Node.js server

The Rust server replaces the Node.js one in place: same configuration files,
command line, wire protocol and console output. Known differences:

- No permessage-deflate. With `ws_compression` set, a warning is logged and
  connections are uncompressed.
- The unused MySQL task store is gone. Tasks go through the storage server.
- `log_timeZone` supports UTC only.
- An invalid `command_plugin` value logs an error instead of stopping the
  server.
- `NODE_EXTRA_CA_CERTS` is still honoured for the AUTH server link.

## S2S Key Generation

Generate Ed25519 keys for server-to-server authentication:

```bash
cd ../andruav_authenticator/scripts
./gen_s2s_keys.sh <server_id>
```

This creates:
- `<server_id>_private.pem` - Private key (keep secret)
- `<server_id>_public.pem` - Public key (share with accepting servers)

See [wiki/S2SAuthentication.md](wiki/S2SAuthentication.md) for complete guide.

## Deployment Configurations

Example configuration files are provided in the `deployment/` directory:

- `server.s2s.super.config` - Parent server configuration
- `server.s2s.drone.config` - Child/drone server configuration

## Security Best Practices

- Enable SSL/TLS for all WebSocket connections
- Use S2S authentication for server-to-server connections
- Restrict file permissions on private keys (0600)
- Use environment variables for sensitive data
- Keep dependencies updated
- Monitor server logs for suspicious activity
- Implement rate limiting for WebSocket connections

## Troubleshooting

### WebSocket Connection Fails
- Check server is running: `./target/release/andruavmainserver`
- Verify port is not in use
- Check SSL certificate validity
- Verify firewall allows WebSocket connections

### S2S Relay Not Working
- Verify `enable_persistant_relay` or `enable_super_server` is set correctly
- Check parent server IP and port configuration
- Ensure S2S authentication is configured on both ends
- Verify public/private key pairs match

### Messages Not Propagating
- Check `_path` array for loop prevention issues
- Verify routing type (`ty`) and target (`tg`) fields
- Review message propagation logs
- Ensure units are in correct groups

## Documentation

- [Message Propagation](wiki/MessagePropagation.md) - Message routing and relay architecture
- [S2S Authentication](wiki/S2SAuthentication.md) - Server-to-server authentication setup
- [Performance Comparison](wiki/PerformanceComparison.md) - Node.js vs Rust architecture and benchmark measurements
- [Live Test on Raspberry Pi 4](wiki/LIVE_TEST_ON_RPI.md) - Real hardware benchmark comparing Node.js vs Rust on airgap.local



## Project Structure

```
droneengage_server/
├── Cargo.toml
├── src/
│   ├── main.rs                # Binary entry point
│   ├── cli.rs                 # Command line, banner, memory watchdog
│   ├── lib.rs                 # Startup sequence
│   ├── config.rs              # server.config (+ .local) loader
│   ├── constants.rs           # Message types, routing constants
│   ├── server.rs              # Shared server state
│   ├── hub.rs                 # Connections, accounts, groups
│   ├── comm_server.rs         # AUTH-driven commands (logins, blocking)
│   ├── chat/                  # WebSocket server, routing, tasks, news, plugins
│   └── s2s/                   # AUTH, storage, relay and parent/child links
├── tests/                     # Integration tests
├── ssl_local/                 # SSL certificates and S2S keys
├── deployment/                # Deployment configurations
├── wiki/                      # Documentation
```

## Contributing

Contributions are welcome! Please read the contributing guidelines before submitting PRs.

## License

[License information here]

## Support

For support and documentation, please refer to [Cloud.Ardupilot.org](https://cloud.ardupilot.org).

[![Ardupilot Cloud EcoSystem](https://cloud.ardupilot.org/_static/ardupilot_logo.png)](https://cloud.ardupilot.org) **Drone Engage** is part of Ardupilot Cloud Eco System




