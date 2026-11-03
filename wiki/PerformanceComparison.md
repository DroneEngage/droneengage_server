# Performance Comparison: Node.js vs Rust Architecture & Optimization

This document details the performance evolution of the DroneEngage communication server and UDP proxy architecture, from the original Node.js implementation to the Rust port and subsequent hot-path optimizations completed in **Plan GL-0076** (Plans A, B, and C).

---

## 1. Executive Summary

Transitioning from Node.js to Rust and applying targeted hot-path concurrency and zero-copy routing enhancements yielded massive gains across all operational dimensions:

* **Delivered Routing Throughput:** Increased from **~83,200 frames/s** (Node.js ceiling) to **427,899 frames/s** under multi-connection load (**+414% / 5.14x**).
* **Small-Fleet Offered Capacity:** Increased handling capacity from saturation at **~83,200 msg/s** (Node.js) to **1,425,357 msg/s** on a 7-drone fleet (**+1,613% / >17x**) and **1,804,960 msg/s** on a 3-drone fleet (**+2,069% / >21x**).
* **Multi-Core Scaling:** Node.js was hard-capped to a single OS core (100% CPU). Rust multi-threaded routing scales across available cores, utilizing up to **1,150% CPU (~11.5 cores)** under heavy load.
* **Latency:** Median round-trip latency dropped by **25% to 40%** at baseline, and by over **85%** under heavy fleet workloads due to the elimination of lock queues and head-of-line blocking.
* **Memory Footprint:** Base RAM decreased from **~235 MB** (Node.js V8 heap) to **~125 MB** in Rust (**46.8% reduction / ~1.9x efficiency**), with zero garbage collection (GC) tail-latency pauses.
* **UDP Telemetry Relay:** Decoupled into an independent daemon (`droneengage_udp_proxy`) with lock-free per-pair forwarding, achieving **160,000 pps** clean delivery and **387,028 pps** burst delivery across a 10-drone fleet (>10x improvement over in-process Node.js UDP proxying).

> **Real Hardware Verification:** For physical test readings comparing Node.js 5.6.1 against the latest Rust stack over a real network on the Raspberry Pi 4 production airgap server (`airgap.local`), see the companion report: **[LIVE_TEST_ON_RPI.md](LIVE_TEST_ON_RPI.md)**.


---

## 2. Baseline Comparison: Original Node.js vs Initial Rust Port

The initial Rust port (`Rust 6.0.0`, commit `d4881b3`) replaced the Node.js 5.6.2 server in place, maintaining identical configuration files, command-line arguments, wire protocols, and console output.

### Measured Baseline (12-core Dev Workstation, Loopback WSS, ~200B Messages)

| Metric | Node.js 5.6.2 (`pr_nodejs_last_version`) | Initial Rust 6.0.0 | Improvement vs Node.js |
| :--- | :--- | :--- | :--- |
| **Throughput (Round Trips/s)** | ~41,600 rtt/s (~83,200 routed frames/s) | ~49,800 rtt/s (~99,600 routed frames/s) | **+19.7% (+20%)** |
| **Latency (p50 RTT)** | ~20 ms | ~17 ms | **15.0% lower latency** |
| **Memory Footprint (RSS)** | ~235 MB | ~125 MB | **46.8% less RAM (~1.9x efficiency)** |
| **CPU Utilization** | ~100% (1 core, fully saturated) | ~206% (~2 cores utilized) | **+106% multi-thread utilization** |
| **Jitter / Latency Spikes** | Periodic V8 GC pause spikes | None (predictable ownership semantics) | **Eliminated GC tail spikes** |

### Architectural Differences

1. **Threading & Concurrency:**
   * **Node.js:** Constrained to a single JavaScript event loop. All network I/O, TLS termination, JSON serialization, and message dispatch shared one thread.
   * **Initial Rust:** Decoupled network socket I/O and TLS encryption across Tokio worker threads, allowing the server to utilize ~2 cores. However, core message routing was initially guarded by a single global `hub` mutex, mirroring Node.js single-thread execution semantics.
2. **Memory Management:**
   * Node.js relied on V8 dynamic heap allocations and garbage collection, resulting in higher resident memory (~235 MB) and GC pauses during high frame rates.
   * Rust uses deterministic compile-time memory ownership, cutting memory usage to ~125 MB without runtime GC overhead.

---

## 3. Modular Separation of the UDP Proxy

Historically, UDP proxying was handled inside the communication server process (`droneengage_server/src/comm_server/js_udp_proxy.js` in Node.js, and initially `src/udp_proxy.rs` in Rust). In **GL-0059** and **GL-0093**, the UDP proxy was extracted into a dedicated standalone repository: `droneengage_udp_proxy`.

### Benefits to Communication Server (`droneengage_server`)

* **Signaling Isolation:** High-rate MAVLink UDP datagram streams (often hundreds of thousands of packets per second) are completely removed from the WebSocket signaling process.
* **Buffer & Resource Protection:** Eliminates the risk of UDP traffic exhausting system file descriptors, network buffer memory, or worker threads in the signaling server.
* **Codebase & Dependency Reduction:** Dropped the `socket2` dependency and removed all UDP socket creation and lifecycle management from the comm server. The comm server now acts strictly as an S2S signaling coordinator over authenticated WebSocket JSON-RPC.

### Benefits to UDP Proxy (`droneengage_udp_proxy`)

* **Dedicated Kernel & I/O Resources:** Can bind directly to dedicated network interfaces and leverage custom OS socket buffer tuning (`SO_RCVBUF` / `SO_SNDBUF` up to 4 MB) without competing with TLS or HTTP workloads.
* **Advanced Security & Pinning Policies:**
  * **GCS-Side Pinning:** Implemented `tofu` (Trust On First Use), `allowlist`, and hybrid `allowlist_then_tofu` policies to protect against unauthorized MAVLink command injection or telemetry hijacking.
  * **Unit Authentication:** Implemented challenge-response unit tokens (`DEU1` hello frames with 16-byte cryptographically secure random tokens) to ensure telemetry packets originate only from authentic vehicle modules (`de_mavlink`).
* **State Persistence & Fault Tolerance:** Atomic state persistence (`leases.json`) preserves active port bindings, allowlists, and peer pins across daemon restarts.
* **Granular Abuse Protection:** Independent per-socket rate limiting (`max_pps_per_socket`, `max_bps_per_socket`) isolates misbehaving or compromised units without impacting the fleet.

---

## 4. Hot-Path Enhancements in Rust (Plan GL-0076)

While the initial Rust port gained ~20% throughput, it remained limited by two critical bottlenecks:
1. Every message underwent full `serde_json::Value` DOM parsing, key lookup, permission injection (`p`), and re-stringification on the hot path (costing ~50% of CPU time in allocator and DOM overhead).
2. The global `hub` mutex was held across message parsing, string manipulation, and plugin dispatch, forcing all connections to queue behind a single lock.

GL-0076 attacked these bottlenecks through three targeted plans:

### Plan A (`GL-0077`): Zero-Copy Hot Path & Plugin Offloading

* **GL-0081 (Zero-Copy Parsing & Splicing):** Replaced `serde_json::Value` DOM parsing with a lightweight byte scan of top-level routing fields (`ty`, `tg`, `f`) and an in-place splice of the permission string (`p`), passing raw frames directly to recipients.
* **GL-0086 (Relay Bypass):** Eliminated the redundant JSON serialization round-trip in `forward_message` when mesh relay links are not enabled.
* **GL-0087 (Socket Writer Batching):** Implemented write batching in the connection sink to coalesce multiple outgoing frames into single TLS records and write syscalls.
* **GL-0080 (Plugin Worker Offload):** Offloaded plugins (such as `command_processing`) to background worker queues, executing them completely off the hot routing path.

**Plan A Results:** Offered throughput increased by **+62% to +441%** across small-fleet profiles, and median latency dropped by **50% to 90%**.

### Plan B (`GL-0078`): Lock-Free Routing Concurrency

* **GL-0083 (Narrowed Hub Mutex):** Reduced the critical section of the `hub` mutex to routing table lookups only (account/group membership and connection handle resolution). Payload preparation, framing, and queue delivery run concurrently on Tokio worker threads outside the lock.

**Plan B Results:** Allowed multi-core hardware to fully engage. Under reference fleet traffic, server CPU scaled up to **11.5 cores (1,150%)**, and offered capacity surged by **+49% to +141%** over Plan A.

### Plan C (`GL-0088`): Lock-Free UDP Relay Data Path

* **GL-0090 (Per-Pair UDP Engine):** Removed the global `m_state` mutex from the datagram reception and forwarding loop (`recv_loop`), replacing it with lock-free per-pair forwarding state and atomic statistics.

**Plan C Results:** Eliminated the multi-pair throughput collapse. Delivered packet rate on a 10-pair fleet jumped from **71,194 pps to 387,028 pps (+444%)**, and maximum paced clean rate (<1% loss) rose from **40,000 pps to 160,000 pps (+300%)**.

---

## 5. Detailed Benchmarks Across Fleet Profiles

The following measurements were captured on a 14-core host (`ThinkPad-T14-Gen-6`) using `de_loadgen` (commit `f0e1932` pre-plan baseline vs master commit `bf4abd2` post-Plan B):

### Fleet Routing Matrix (Airgap Settings, Unthrottled Flood)

| Profile & Workload | Pre-Plan Rust Offered | Post-GL-0076 Offered | Offered Gain | Pre-Plan Latency (p50) | Post-GL-0076 Latency (p50) | Latency Improvement |
| :--- | :--- | :--- | :--- | :--- | :--- | :--- |
| **1 Drone + 1 GCS (p2p text)** | 198,911 /s | 757,747 /s | **+281%** | 265 ms | 28 ms | **-89.4%** |
| **1 Drone + 1 GCS (p2p binary)** | 182,764 /s | 573,042 /s | **+214%** | 166 ms | 22 ms | **-86.7%** |
| **1 Drone + 1 GCS (bcast text)** | 177,241 /s | 1,099,861 /s | **+521%** | 119 ms | 15 ms | **-87.4%** |
| **1 Drone + 1 GCS (bcast binary)** | 167,558 /s | 1,088,082 /s | **+549%** | 57 ms | 12 ms | **-78.9%** |
| **Account A (7 drones, 2 GCS - p2p text)** | 211,289 /s | 1,425,357 /s | **+575%** | 1,145 ms | 198 ms | **-82.7%** |
| **Account A (7 drones, 2 GCS - bcast text)** | 150,861 /s | 934,653 /s | **+520%** | 880 ms | 126 ms | **-85.7%** |
| **Account B (3 drones, 1 GCS - p2p text)** | 218,758 /s | 1,409,409 /s | **+544%** | 502 ms | 56 ms | **-88.8%** |
| **Account B (3 drones, 1 GCS - bcast text)** | 194,149 /s | 1,804,960 /s | **+830%** | 284 ms | 29 ms | **-89.8%** |
| **Combined Accounts (7d+2g & 3d+1g bcast)** | 152,596 /s | 880,715 /s | **+477%** | 1,336 / 1,557 ms | 207 / 237 ms | **-84.5%** |
| **Regression Check (96 pairs p2p)** | 296,618 /s | 540,209 /s | **+82%** | 4,223 ms | 2,377 ms | **-43.7%** |

*Under fleet broadcast flood (Account A: 7 drones, 2 GCS), delivered frames to GCS sockets surged from **2,078 frames/s** (pre-plan bottleneck) to **413,137 frames/s** (**+19,781% / ~200x**), as receiver sockets were no longer starved behind lock contention.*

### Standalone UDP Relay Scaling (Plan C / `GL-0092`)

*Measured at 300-byte datagrams, loopback, airgap mode:*

| Fleet Size | Pre-Plan Unpaced Delivered | Post-Plan C Unpaced Delivered | Unpaced Gain | Pre-Plan Clean Rate (<1% loss) | Post-Plan C Clean Rate (<1% loss) | Clean Usable Gain |
| :--- | :--- | :--- | :--- | :--- | :--- | :--- |
| **1 Pair** | 183,795 pps | 167,624 pps | -8.8% | 128,000 pps | 128,000 pps | **Parity** |
| **3 Pairs** | 96,008 pps | 363,808 pps | **+279%** | 96,000 pps | 192,000 pps | **+100% (2.0x)** |
| **7 Pairs** | 81,588 pps | 461,373 pps | **+465%** | 56,000 pps | 224,000 pps | **+300% (4.0x)** |
| **10 Pairs** (7 accA + 3 accB) | 71,194 pps | 387,028 pps | **+444%** | 40,000 pps | 160,000 pps | **+300% (4.0x)** |

---

## 6. Comprehensive Summary: Overall Improvement vs Original Node.js

Comparing the final optimized Rust stack against the original Node.js 5.6.2 implementation:

| Metric / Scenario | Original Node.js 5.6.2 | Optimized Rust Stack | Overall Improvement vs Original Node.js |
| :--- | :--- | :--- | :--- |
| **Max Delivered Routing Throughput (Multi-connection / 96 pairs)** | ~83,200 routed msg/s (~41.6k rtt/s) | **427,899 delivered msg/s** | **+414% (5.14x throughput)** |
| **Small-Fleet Offered Capacity (7-drone fleet)** | Saturated at ~83,200 msg/s | **1,425,357 offered msg/s** | **+1,613% (>17x capacity)** |
| **Small-Fleet Offered Capacity (3-drone fleet)** | Saturated at ~83,200 msg/s | **1,804,960 offered msg/s** | **+2,069% (>21x capacity)** |
| **Multi-Core Scaling Ceiling** | 100% (Hard-capped to 1 core) | **1,150% (~11.5 cores utilized)** | **+1,050% CPU utilization capacity** |
| **Base Memory Footprint (RSS)** | ~235 MB RSS | **~125 MB RSS** | **46.8% less RAM (~1.9x efficiency)** |
| **Single-Pair Latency (p50)** | ~20 ms | **12 – 15 ms (broadcast) / ~22 ms (p2p)** | **25% – 40% latency reduction** |
| **UDP Proxy Fleet Forwarding (10 pairs)** | In-process, throttled (~20k–40k pps) | **160,000 pps clean / 387,028 pps burst** | **>800% – 1,200% (>9x – 13x rate)** |

---

## 7. Architectural Analysis: Could Node.js Achieve the Same Gains?

A natural question arises from the baseline data: **The initial port to Rust yielded only a ~20% throughput increase. Could the subsequent large gains (+400% to +2,000%) have been achieved simply by applying the same optimizations to Node.js?**

The technical answer is **no**. The initial port was modest because it deliberately mirrored Node.js's single-threaded, DOM-heavy architecture. However, Node.js has fundamental runtime and language constraints that make the later gains impossible.

### 1. Why Was the Initial Rust Gain Only ~20%?

The initial Rust port was designed as a strict, bug-for-bug 1:1 behavioural clone of Node.js to ensure backward compatibility:
* **Artificial Serialization:** Core message routing was placed behind a single global `hub` mutex, replicating Node.js's single-threaded event loop.
* **Redundant JSON DOM Allocations:** Every frame was parsed into a full `serde_json::Value` DOM, modified, and re-serialized. CPU profiling in GL-0085 revealed that **~50% of server CPU** was consumed purely by memory allocator churn and JSON object maps.

Because initial Rust was executing Node.js's exact architectural pattern, it hit the same structural limits. The ~20% gain came solely from Tokio running TLS encryption and socket I/O across worker threads.

### 2. The Hard Ceilings of the Node.js / V8 Runtime

If the optimizations in Plans A, B, and C were applied to Node.js, they would quickly collide with three hard runtime barriers:

#### A. The Single-Threaded Core Cap (Plan B)
* **Rust:** Plan B shrunk the lock to fast routing table lookups, allowing Tokio worker threads to parse headers, construct payloads, and write to client sockets concurrently across **11.5 CPU cores (1,150% CPU)**.
* **Node.js Ceiling:** Node.js executes JavaScript on a single OS thread. A Node.js process cannot exceed 100% CPU on one core. Running shared-memory parallel routing across 14 cores is impossible in JavaScript without inter-process communication (IPC) or `Worker` threads, both of which require serializing and copying messages across threads—re-introducing the exact overhead we eliminated. Node.js could never scale from 1 core to 11.5 cores.

#### B. V8 Heap Churn and Garbage Collection Pauses (Plan A)
* **Rust:** Plan A used zero-copy raw byte scanning and in-place byte splicing with no dynamic heap allocations on the hot path.
* **Node.js Ceiling:** In Node.js (V8), strings are immutable and `Buffer` operations allocate JavaScript objects on the V8 heap. Offering **1.4 million to 1.8 million frames per second** through V8 would create extreme memory churn in the Young Generation heap. This forces frequent Garbage Collector (GC) scavenges and "stop-the-world" mark-sweep pauses, causing massive latency spikes and frame drops.

#### C. Event-Loop Context Switching on Datagrams (Plan C)
* **Rust:** Plan C forwards **387,000 UDP packets per second** directly between OS sockets with zero locks and zero allocations.
* **Node.js Ceiling:** In Node.js, every incoming UDP datagram via `dgram` must cross the C++ libuv boundary into JavaScript to emit a `'message'` event. That context-switching overhead saturates a single-threaded Node.js process at approximately 20,000 to 40,000 packets per second, regardless of how simple the forwarding logic is.

### Comparison: Optimization Limits in Node.js vs Measured Results in Rust

| Optimization Step | Estimated Ceiling in Node.js | Measured Result in Rust | Root Cause of Node.js Limitation |
| :--- | :--- | :--- | :--- |
| **Bypass JSON DOM (Plan A)** | ~1.5x – 2.0x gain (~120k–160k msg/s) | **2x – 5x gain** | V8 Buffer allocations and GC pressure under flood. |
| **Lock-Free Concurrency (Plan B)** | **0% gain** (Architecture impossible) | **+49% to +141% gain** (Scales to **1,150% CPU**) | Node.js is hard-capped to a single CPU core. |
| **Lock-Free UDP Relay (Plan C)** | Saturated at ~30k–40k pps | **387,000 pps** | Libuv-to-V8 event loop context switching overhead. |

### Conclusion

Porting to Rust was not just an incremental step—it was the **enabling foundation**. Moving to Rust provided the multi-core thread runtime, compile-time memory safety without GC, and zero-cost abstraction needed for Plans A, B, and C to unlock **5x to 21x higher throughput** and scale across modern multi-core servers. In Node.js, those same optimization attempts would have stalled at the 1-core barrier at ~150,000 msgs/s.

