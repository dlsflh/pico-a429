# Stage 8 Report: Intermediate Zoom Out Review

*   **Target Document**: `docs/designs/pure_rust_host_and_mcp_server.md` (lines 1..506)
*   **Reviewer**: `egm-zoom-out` (Skeptical Architect)
*   **Status**: Complete

---

## Executive Summary

The intermediate zoom-out audit synthesizes findings across Stages 5–7 and performs a high-altitude architectural altitude check. The core thesis—transitioning to a pure-Rust monorepo sharing `crates/arinc429-core`—is validated as sound and optimal.

Crucially, this review **de-escalated false blockers** (such as documentation typos and stylistic choices) and **rejected premature over-engineering** (specifically rejecting the proposed firmware `EXPEDITED_TX_SLOT`). It affirmed five genuine architectural/physical blockers and eight high-value systemic simplifications that ensure physical determinism, zero credit deadlock, and robust AI agent integration.

---

## YAGNI & Essential Simplicity Evaluation of Proposed Remediations

| Proposed Remediation | Origin | Skeptical Architectural Evaluation | Recommendation |
| :--- | :--- | :--- | :--- |
| **Dedicated 1-word `EXPEDITED_TX_SLOT` per channel in firmware** | ZO-01, CR-02 | ❌ **REJECT (Over-Engineering / YAGNI Anti-Pattern)**.<br>The document conflated Control Plane (`CMD_REQ`: ping, configure, abort) with Data Plane (`send_word`). In firmware, `CMD_REQ` is processed immediately in `execute_binary_command` and *never enters* `CHANNEL_TX_QUEUES`. ARINC 429 is a strictly sequential waveform; injecting an out-of-band priority word mid-stream corrupts sequence integrity. When no stream runs, FIFO is empty and transmission takes < 360 µs. | **REJECT firmware slot**.<br>Define Lane 0 strictly as **Control Plane Only** (`CMD_REQ`). Data transmission (`send_word` and `stream_send`) belongs to Data Plane. For cancellation, provide `CMD_REQ: ABORT_TX` that clears the channel queue. |
| **Relative Credit Baseline Latching on Host Bootstrap** | ZO-03 | ✅ **ACCEPT (Essential Simplicity / Bug Fix)**.<br>$\text{InFlight} = \text{SentCount} - \text{CumulativeConsumed}$ underflows in unsigned 32-bit math ($0 - 50,000 = 4,294,917,296$) upon host restart, permanently deadlocking credits. Latching baseline on the first credit frame is a simple host-side fix. | **ACCEPT**.<br>Implement in `crates/pico429/src/transport/scheduler.rs`. |
| **Atomic Bitmask Credit Coalescing** | ZO-04, CR-08 | ✅ **ACCEPT (Essential Simplicity / Reduces Firmware Complexity)**.<br>Sending 522 unbatched 16-byte `STREAM_CREDIT` frames/s chokes USB Full Speed transaction slots and silently drops `CMD_ACK` frames on queue overflow. An `AtomicU16` dirty bitmask lets `usb_cdc_tx_task` pack credits directly into 64-byte batch buffers, eliminating queue memory and packet flooding. | **ACCEPT**.<br>Replaces multi-frame queue machinery with a single atomic integer. |
| **Baud-Rate Watermark Scaling & Tail-Flush Trigger** | CR-06, Comprehension-5 | ✅ **ACCEPT (Essential Simplicity / Physical Correctness)**.<br>A fixed 32-word threshold with 32-word ceiling at 12.5 kbps guarantees buffer starvation. Bursts with $< 32$ words never reach the threshold, permanently leaking host credits. | **ACCEPT**.<br>Trigger credit when consumed $\ge \min(32, \text{ceiling}/2)$ **OR** when `queue.is_empty()`. |
| **Host Current Value Table (CVT) `[12][256]`** | ZO-06 | ✅ **ACCEPT (High Value / Low Complexity for AI Agents)**.<br>At 16.6k words/s, a 32k sequential log flushes in ~2 seconds. LLM agent reasoning takes 10–20 seconds. A flat host table of `[AtomicU32; 256]` per channel occupies only 12 KB RAM and provides $O(1)$ instant state inspection. | **ACCEPT**.<br>Keep as a flat in-memory array in `crates/pico429/src/telemetry/cvt.rs`. |

---

## Consolidated Findings Matrix

### 🔴 Blocker (Genuine Architectural & Physical Contradictions)

1. **Control vs Data Plane Preemption Separation**:
   * *Contradiction*: Claiming Lane 0 provides `< 1.0 ms` latency for single data words (`send_word`) by bypassing credit flow control causes underflow panic in host accounting when words enter firmware queues uncredited.
   * *Resolution*: Restrict Lane 0 to **Control Plane Only** (`CMD_REQ`: status, configure, ping, abort). Move `pico429_send_word` to Data Plane with normal credit gating (or reject with `ERR_MODE_CONFLICT` if a stream is running on that channel). Implement `CMD_REQ: ABORT_TX` to drain/clear channel FIFOs on demand.
2. **Mathematical Latency Contradiction (< 2.0 ms vs 34.56 ms Runway)**:
   * *Contradiction*: Asserting `< 2.0 ms` queue latency while holding a $34.56\text{ ms} \sim 46.08\text{ ms}$ (96–128 words) buffer runway in firmware is physically impossible for words entering the tail of the queue.
   * *Resolution*: Decouple metrics in the specification:
     * Bulk Streaming In-Flight Transit Delay: $34.56\text{ ms} \sim 46.08\text{ ms}$ (anti-starvation buffer runway).
     * Host USB Batch Dispatch Latency: $< 1.0\text{ ms}$.
     * Interactive Control Command Latency: $< 1.0\text{ ms}$ round-trip.
3. **Credit Desynchronization and Underflow on Host Lifecycle Reconnect**:
   * *Contradiction*: Monotonic tracking $\text{InFlight} = \text{SentCount} - \text{CumulativeConsumed}$ fails when host restarts (`SentCount = 0`) while firmware has already transmitted words ($\text{CumulativeConsumed} > 0$), resulting in underflow ($4,294,917,296$) and permanent credit lockout.
   * *Resolution*: Relative baseline latching: on bootstrap `STREAM_CREDIT`, latch `BaselineConsumed = ReportedConsumed`. Compute $\text{InFlight} = \text{SentCount} - (\text{ReportedConsumed} - \text{BaselineConsumed})$.
4. **Permanent Credit Leakage on Dropped Stream Frames**:
   * *Contradiction*: If firmware drops a `STREAM_TX_DATA` frame (e.g. queue full or channel disabled), the word never enters the PIO FIFO and never increments `CumulativeConsumed`. That credit is permanently lost on the host.
   * *Resolution*: Track `TotalHandled = ConsumedCount + DroppedCount` in firmware, or report `CumulativeDropped` in `STREAM_CREDIT` so host reconciles in-flight accounting.
5. **Low-Speed (12.5 kbps) Starvation & Sparse Stream Deadlock**:
   * *Contradiction*: Static 32-word watermark with 32-word ceiling at 12.5 kbps leaves 0 ms runway. Sparse streams ($< 32$ words) never cross threshold, freezing credits.
   * *Resolution*: Scale watermark to $\min(32, \text{ceiling}/2)$ (8 words at 12.5k) and trigger credit immediately when `queue.is_empty()`.

---

### 🟡 Major (System Integrity, Protocol Contracts & Performance Risks)

6. **Wire Protocol Magic & Field Layout Typo**: Align `Magic (0xA429)` to `0x4129` (`[0x41, 0x29]`) and status code to Byte 3 `flags`.
7. **Firmware USB CDC 100 ms Timeout**: Increase write timeout in `usb_service.rs` to $\ge 300\text{ ms}$ to honor the 245 ms absorption guarantee.
8. **Control ACK Starvation & USB Packet Flooding**: Use `AtomicU16` dirty bitmask to coalesce credits into 64-byte batch transfers; reserve `USB_CMD_RESP_QUEUE` exclusively for `CMD_ACK`.
9. **Missing ARINC 429 Word Codec in Core Implementation Plan**: Add `crates/arinc429-core/src/word.rs` to the implementation plan.
10. **AI Telemetry Ring Buffer Overwrite**: Add host Current Value Table (CVT) `[AtomicU32; 256]` per channel and expose tool `pico429_get_latest_labels`.
11. **Premature `{job_id}` Polling**: Support synchronous execution by default with `timeout_ms`; return `{job_id}` only for unbounded streams.
12. **Monorepo Profile Override & Verification Package Name**: Add `[profile.release.package.pico429] opt-level = 3` to `Cargo.toml`. Correct package name to `pico-429-firmware`.
13. **Serial Port Concurrency Boundary**: Document exclusive COM port ownership by MCP server.

---

### 🟢 Nit (Cosmetic, Documentation, Layout & Phrasing)

14. **Stylistic Hyperbole & Metric Drift**: Change "absolute immunity" to "bounded jitter tolerance up to 34.56 ms"; harmonize CPU target to `< 0.5% CPU load`.
15. **Document Structure & Formatting**: Promote `Additional Resources` and `Future Improvements` out of `Non-Functional Requirements` to H2; remove Chinese parentheticals from headings; escape `<u8>` in table.
16. **Dependency Version Pinning**: Update `rmcp` dependency specification to `rmcp = { version = "3.9", features = ["server", "transport-stdio"] }`.
17. **Hardware Initialization Detail**: Drive GPIO 23 (`WL_ON`) low during firmware startup.
18. **Host Syscall Optimization**: Allow host serial write buffering up to 512 bytes rather than enforcing 64-byte chunks.
