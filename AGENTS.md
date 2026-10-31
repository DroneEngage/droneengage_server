# AGENTS.md — droneengage_server

Andruav/DroneEngage communication (signaling) server — WebSocket
messaging between units and GCSs, parent/child s2s mesh relay, system
commands, storage integration. **Rust (tokio)** port of the Node.js
server (old code on `pr_nodejs_last_version`).

## Quickstart

    cargo build --release     # -> target/release/andruavmainserver (no plugins)
    cargo test                # unit + integration tests

## Index — `.agents/wiki/`

| File | What it covers |
|---|---|
| [`agents_overview.md`](.agents/wiki/agents_overview.md) | plugins-workspace binary + run dir, command_plugin config, source layout |

See `../../AGENTS.md` for the workspace and shared server conventions.
