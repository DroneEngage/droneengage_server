# Comm-server routing benchmark

Measures the hot path of the communication server: WebSocket message
routing between parties (JSON parse → permission inject → re-serialize →
route → forward), end-to-end over WSS.

`routing_benchmark.py` is a closed-loop load client: N unit↔GCS pairs log
in through the `de_auth` frame (`local_server_enabled`, so no AUTH needed),
each unit keeps K messages in flight to its GCS, the GCS echoes back, and
the client reports routed msg/s plus RTT percentiles. Because it is
closed-loop it cannot flood the `outbound_guard` — an unbounded blast just
fills the 4 MB hard cap and gets sockets killed.

Needs only `python3` + `websockets` (already used by droneengage_studio).

## Run against the Rust server

From the repo root (so `./ssl_local` resolves):

    cargo build --release
    target/release/andruavmainserver --config=tests/bench/server.config.bench &

    python3 tests/bench/routing_benchmark.py --port 19966 --pairs 12 --inflight 20 --secs 8

To push past one client process, run several with different `--prefix`
(sender ids must not collide):

    for p in a b c d; do python3 tests/bench/routing_benchmark.py \
        --port 19966 --pairs 12 --inflight 20 --secs 8 --prefix $p & done

Sum the per-process `THROUGHPUT` lines for the aggregate rate.

## Compare with the legacy Node.js server

    git worktree add /tmp/de_nodejs pr_nodejs_last_version
    cd /tmp/de_nodejs && npm install
    # copy the same override keys into server.config.local (see
    # server.config.bench), changing "server_port" to e.g. 19967
    node ./src/server.js &

    python3 tests/bench/routing_benchmark.py --port 19967 --pairs 12 --inflight 20 --secs 8

## Reference numbers (dev workstation, 12 cores, loopback WSS, ~200B msgs)

| Server | msg/s routed | server CPU | RTT p50 | RSS |
|---|---|---|---|---|
| Node.js 5.6.2 (`pr_nodejs_last_version`) | ~41.6k | ~100% (1 core, saturated) | ~20 ms | ~235 MB |
| Rust 6.0.0 | ~49.8k | ~206% | ~17 ms | ~125 MB |

Node.js is hard-capped at one core; the Rust server routes under a global
hub mutex (same semantics) but runs socket I/O and TLS on other cores —
~1.2× throughput, ~2× less memory, and no GC tail spikes.
