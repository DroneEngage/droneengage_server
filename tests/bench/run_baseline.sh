#!/usr/bin/env bash
# GL-0085 baseline suite: the small-fleet profiles of megaplan GL-0076
# (one drone with one GCS; account A alone with 7 drones and 2 GCS;
# account B alone with 3 drones and 1 GCS; both accounts together), each
# one-to-one and broadcast, text and binary, drones unthrottled — plus one
# many-connection run as the regression check.
#
# Run from the repo root:
#   cargo build --release
#   (cd tests/bench/loadgen && cargo build --release)
#   tests/bench/run_baseline.sh | tee baseline_$(git rev-parse --short HEAD).log
#
# Starts and stops the comm server itself (fleet config for the account
# runs, local config for the regression run).

set -u
cd "$(dirname "$0")/../.."

SECS="${SECS:-8}"
LOADGEN=tests/bench/loadgen/target/release/de_loadgen
SRV=target/release/andruavmainserver

srv_start() {
    "$SRV" --config="$1" >/tmp/de_bench_srv.log 2>&1 &
    SRV_PID=$!
    for _ in 1 2 3 4 5 6 7 8 9 10; do
        kill -0 "$SRV_PID" 2>/dev/null || break
        ss -tln | grep -q ':19966 ' && return 0
        sleep 0.5
    done
    echo "!!! comm server failed to start on :19966 — log:" >&2
    tail -15 /tmp/de_bench_srv.log >&2
    exit 1
}

srv_stop() {
    kill "$SRV_PID" 2>/dev/null
    wait "$SRV_PID" 2>/dev/null
    sleep 0.4
}

run() {
    echo "===== $* ====="
    timeout 120 "$LOADGEN" --server-pid "$SRV_PID" --secs "$SECS" "$@"
    echo
}

echo "### baseline suite $(date -u +%Y-%m-%dT%H:%M:%SZ) commit $(git rev-parse --short HEAD) host $(hostname)"

srv_start tests/bench/server.config.fleet

for FLEET in "acc1:1:1" "accA:7:2" "accB:3:1" "accA:7:2,accB:3:1"; do
    for MODE in p2p broadcast; do
        for FRAMES in "" "--binary"; do
            run --auth 127.0.0.1:19970 --fleet "$FLEET" --mode "$MODE" $FRAMES
        done
    done
done

srv_stop
srv_start tests/bench/server.config.bench

echo "===== many-connection regression: --local --pairs 96 --mode p2p ====="
run --local --pairs 96 --mode p2p

srv_stop
echo "### done"
