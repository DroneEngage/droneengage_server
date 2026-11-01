#!/usr/bin/env python3
"""Routing throughput/latency benchmark for the communication server.

Closed-loop test: N unit<->GCS pairs log in with the de_auth frame
(local_server_enabled), each unit keeps K messages in flight routed to its
GCS (ty:'i', tg:<gcs>), the GCS echoes back, the unit measures round-trip
latency. Reports routed msg/s and RTT percentiles.

Works against both the Rust server and the legacy Node.js server
(pr_nodejs_last_version) — same wire protocol.

    python3 tests/bench/routing_benchmark.py --port 19966 --pairs 12 --inflight 20 --secs 8

Needs: python3 + websockets. The server must run with
local_server_enabled=true (see server.config.bench in this folder).
"""
import argparse
import asyncio
import re
import ssl
import time

import websockets

PAYLOAD = ',"lat":30.0444,"lng":31.2357,"alt":112.5,"hdg":270,"sat":14,"fix":3,"s1":88,"s2":77,"b":"%s"' % ("x" * 64)
RT_RE = re.compile(r'"rt":(\d+)')
running = True


def tls_context():
    ctx = ssl.create_default_context()
    ctx.check_hostname = False
    ctx.verify_mode = ssl.CERT_NONE
    return ctx


async def login(port, sender_id, actor_type):
    ws = await websockets.connect(f"wss://127.0.0.1:{port}", ssl=tls_context())
    await ws.send('{"ty":"s","mt":"de_auth","f":"x","s":"%s","at":"%s"}' % (sender_id, actor_type))
    async for raw in ws:
        if '"ok"' in raw:
            return ws
        if '"fail"' in raw:
            raise RuntimeError(f"auth rejected for {sender_id}")


def routed_msg(target_id, sender_id):
    return '{"ty":"i","tg":"%s","sd":"%s","mt":9999,"ms":{"rt":%d%s}}' % (
        target_id, sender_id, int(time.time() * 1000), PAYLOAD)


async def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--port", type=int, required=True)
    ap.add_argument("--pairs", type=int, default=20)
    ap.add_argument("--inflight", type=int, default=10)
    ap.add_argument("--secs", type=int, default=8)
    ap.add_argument("--prefix", default="", help="sender-id prefix (run several clients in parallel)")
    args = ap.parse_args()

    latencies = []
    counters = {"routed": 0, "echoes": 0}
    tasks = []
    senders = []

    async def gcs_loop(gcs_ws, unit_id, gcs_id):
        async for raw in gcs_ws:
            counters["routed"] += 1
            m = RT_RE.search(raw)
            if m:
                await gcs_ws.send('{"ty":"i","tg":"%s","sd":"%s","mt":9999,"ms":{"rt":%s}}'
                                  % (unit_id, gcs_id, m.group(1)))

    async def unit_loop(unit_ws, unit_id, gcs_id):
        async for raw in unit_ws:
            m = RT_RE.search(raw)
            if m:
                counters["echoes"] += 1
                latencies.append(int(time.time() * 1000) - int(m.group(1)))
                if running:
                    await unit_ws.send(routed_msg(gcs_id, unit_id))

    for i in range(args.pairs):
        gcs_id, unit_id = f"g_{args.prefix}{i}", f"u_{args.prefix}{i}"
        gcs_ws = await login(args.port, gcs_id, "g")
        unit_ws = await login(args.port, unit_id, "d")
        tasks.append(asyncio.ensure_future(gcs_loop(gcs_ws, unit_id, gcs_id)))
        tasks.append(asyncio.ensure_future(unit_loop(unit_ws, unit_id, gcs_id)))
        senders.append((unit_ws, unit_id, gcs_id))

    print(f"{args.pairs} pairs connected, {args.inflight} in-flight each", flush=True)
    start = time.time()
    for unit_ws, unit_id, gcs_id in senders:
        for _ in range(args.inflight):
            await unit_ws.send(routed_msg(gcs_id, unit_id))
    await asyncio.sleep(args.secs)
    global running
    running = False
    await asyncio.sleep(1.0)
    for t in tasks:
        t.cancel()

    elapsed = time.time() - start - 1.0
    latencies.sort()
    q = lambda p: latencies[min(len(latencies) - 1, int(len(latencies) * p))] if latencies else "n/a"
    print(f"routed={counters['routed']} echoes={counters['echoes']} drops={counters['routed'] - counters['echoes']}")
    print(f"THROUGHPUT {counters['echoes'] / elapsed:.0f} msg/s routed "
          f"RTT p50={q(0.5)}ms p95={q(0.95)}ms p99={q(0.99)}ms max={latencies[-1] if latencies else 'n/a'}ms")


if __name__ == "__main__":
    asyncio.run(main())
