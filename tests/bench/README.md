# Comm-server routing benchmark

Measures the hot path of the communication server: WebSocket message
routing between parties (JSON parse → permission inject → re-serialize →
route → forward), end-to-end over WSS.

## de_loadgen — the fleet load client

`loadgen/` is a small Rust binary (tokio + tokio-tungstenite, reusing
`droneengage_server_common`) built because a single Python process topped
out near 7.6k round trips/s at 100% CPU — the client was the limit, not
the server. One `de_loadgen` process drives a whole small fleet over WSS
while using well under one core itself.

Build:

    (cd tests/bench/loadgen && cargo build --release)

### Fleet and login modes

Two ways to log in:

- `--local` — the old `local_server_enabled` path (server.config.bench):
  every socket lands in one account/group. Single-account runs only.
- `--auth IP:PORT` — the real login path (server.config.fleet): the tool
  runs a stub AUTH the comm server dials into (`s2s_ws_target`), announces
  a login per party, and each party presents the temp key it got back.
  Several accounts on one server, each in its own group — the way real
  fleets look to the router.

The fleet is `--fleet "acc:drones:gcs[,acc:drones:gcs]"` (per-account
account id, drone count, GCS count, optional group id), `--pairs N`
(N drones + N GCS in one account), or `--account A --drones N --gcs M`.

### Workload modes

- `--mode p2p` — each drone sends `ty:'i'` one-to-one to a GCS of its
  account (`tg`), the GCS echoes it back (`mt` 9999). Measures delivered
  frames at the GCS and round trips at the drone.
- `--mode broadcast` — each drone sends `ty:'i'` with no `tg` (the normal
  telemetry route: drone → GCS of its group). Measures delivered frames
  and one-way latency at every GCS.
- `--binary` — the same header routed as a WS binary frame
  (`JSON \0 payload`, `mt` 6502 instead of 2022).
- `--rate N` — pace each drone at N frames/s; `0` (default) = as fast as
  the sockets take them. `--secs`, `--msg-bytes`, `--prefix` as expected.

### What the output counts

- `sent` — frames the drones offered to the server.
- `delivered` — frames a GCS socket received (drops from the
  outbound_guard's latest_wins skips are the gap vs `sent`).
- `rounds` — echo replies that completed a round trip (p2p only).
- `violations` — frames a receiver saw from a sender outside its account
  (the account-isolation check; should be 0).
- `stalls` — drone sends that waited on a full client-side queue (a
  client-side limit indicator; a run with heavy stalls is suspect).
- `client cpu` / `server cpu` — % of one core from /proc ticks
  (`--server-pid` for the server).

Example — the human's reference fleet on one server:

    target/release/andruavmainserver --config=tests/bench/server.config.fleet &
    tests/bench/loadgen/target/release/de_loadgen --auth 127.0.0.1:19970 \
        --fleet "accA:7:2,accB:3:1" --mode broadcast --secs 10 --server-pid $!

`run_baseline.sh` runs the standard suite (all fleet profiles ×
p2p/broadcast × text/binary, plus a 96-pair regression run).

## routing_benchmark.py — the legacy client

`routing_benchmark.py` is still the closed-loop check against the legacy
Node.js server (same wire protocol): N unit↔GCS pairs log in through the
`de_auth` frame (`local_server_enabled`), each unit keeps K messages in
flight to its GCS, the GCS echoes back. Needs only `python3` +
`websockets`.

**What its THROUGHPUT line means:** it counts `echoes` — completed round
trips — under the label "routed". A round trip crosses the router about
twice (unit→GCS, GCS→unit), so "routed msg/s" ≈ 2× the THROUGHPUT figure.
It also reports only one-to-one `ty:'i'` traffic, which never reaches the
mesh-forward path. On a fast machine the Python process itself saturates
before the server does — use de_loadgen for server-side numbers.

    python3 tests/bench/routing_benchmark.py --port 19966 --pairs 12 --inflight 20 --secs 8

## Reference numbers (dev workstation, 12 cores, loopback WSS, ~200B msgs)

| Server | round trips/s | server CPU | RTT p50 | RSS |
|---|---|---|---|---|
| Node.js 5.6.2 (`pr_nodejs_last_version`) | ~41.6k* | ~100% (1 core, saturated) | ~20 ms | ~235 MB |
| Rust 6.0.0 | ~49.8k* | ~206% | ~17 ms | ~125 MB |

\* as printed by the old client: completed round trips/s, ≈ two routed
frames each — see "What its THROUGHPUT line means" above. These came from
many-process runs on a different machine; the GL-0076 baseline (recorded
in the megaplan's notes) is the reference for later comparisons.

Node.js is hard-capped at one core; the Rust server routes under a global
hub mutex (same semantics) but runs socket I/O and TLS on other cores.
