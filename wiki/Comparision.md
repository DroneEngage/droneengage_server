Here is a numerical comparison of **C, Rust, Node.js, and Python**.

*Note: For **Execution Speed**, **C** is set as the baseline (1.0x). Higher numbers mean the language takes longer to run CPU-bound tasks.*

| Metric | C | Rust | Node.js (JavaScript) | Python |
| --- | --- | --- | --- | --- |
| **Execution Speed (CPU-bound)** | **1.0x** (Fastest baseline) | **1.0x – 1.1x** (Near-native speed) | **2x – 5x** slower than C | **10x – 50x+** slower than C |
| **Memory Footprint (Base RAM)** | **~2 – 5 MB** | **~5 – 10 MB** | **~30 – 50 MB** | **~20 – 40 MB** |
| **Startup Time** | **< 1 ms** (Instant) | **< 5 ms** (Instant) | **~50 – 100 ms** | **~20 – 50 ms** |
| **Garbage Collector (GC)** | No (Manual memory) | No (Ownership/Borrow checker) | Yes (V8 GC pauses) | Yes (Reference counting + GC) |
| **Concurrency Model** | Manual / OS Threads | Fearless Concurrency / Async (Tokio) | Event Loop (Single-threaded I/O) | GIL (Global Interpreter Lock) / Async |

---

For real measured benchmark data, architectural differences, and percentage improvements between the original Node.js communication server and the optimized Rust implementation (Plan GL-0076), see [PerformanceComparison.md](PerformanceComparison.md).

