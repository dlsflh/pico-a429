# Stage 5 Report: Initial Zoom Out Review & Inventive Problem-Solving

*   **Target Document**: `docs/designs/pure_rust_host_and_mcp_server.md` (lines 1..506)
*   **Reviewers**: `egm-zoom-out` (Skeptical Architect), `egm-inventive-catalyst` (TRIZ Inventive Catalyst)
*   **Status**: Complete

---

## Executive Summary

The strategic shift to a pure-Rust workspace crate sharing [`arinc429-core`](file:///c:/Users/lihan/Work/pico-429/crates/arinc429-core/src/protocol.rs) eliminates opcode drift, serializes data with zero heap allocations, and aligns cleanly with the monorepo architecture.

However, cross-layer architectural auditing across the host transport model and embedded firmware realities identified **three critical architectural contradictions (Blockers)**:
1. **FIFO Preemption Contradiction**: Host Lane 0 expedites control frames, but firmware routes both priority words and bulk streams into the same 128-word FIFO queue (`CHANNEL_TX_QUEUES`), where a priority word sits behind 34.56 ms of queued stream words.
2. **Mathematical Contradiction in Latency Claims**: The document claims `< 1.0 ms` / `< 2.0 ms` host-to-bus queue latency while deliberately maintaining a 96-to-128 word ($34.56\text{ ms} \sim 46.08\text{ ms}$) anti-jitter queue runway. Bulk stream queue residence time and expedited control latency must be explicitly decoupled.
3. **State Desynchronization on Host Reconnect**: Reconnection with `SentCount = 0` against a running firmware counter causes unsigned 32-bit underflow ($0 - 50,000 = 4,294,917,296$), permanently locking flow control credits at zero.

Additionally, **five major operational risks** were identified, including silent dropping of `CMD_ACK` frames under high credit traffic, missing word codecs in `arinc429-core`, telemetry eviction during slow AI agent turns, and premature `{job_id}` polling.

---

## Findings Matrix

| ID | Priority | Dimension | Summary of Architectural Finding | Actionable Resolution |
| :--- | :--- | :--- | :--- | :--- |
| **ZO-01** | 🔴 **Blocker** | Firmware Queue Preemption | Priority words wait behind 96–128 stream words in firmware FIFO; stream abort leaves stale words in buffer. | Add 1-word `EXPEDITED_TX_SLOT` per channel; add `ABORT_TX = 0x0B` opcode to flush channel FIFO. |
| **ZO-02** | 🔴 **Blocker** | Latency vs Runway Physics | Mathematical impossibility of claiming `< 2.0 ms` queue latency while holding a $34.56\text{ ms}$ anti-jitter runway. | Decouple metrics: Bulk stream queue residence = $34 \sim 46\text{ ms}$; Expedited command latency = $< 1.0\text{ ms}$. |
| **ZO-03** | 🔴 **Blocker** | Lifecycle Desynchronization | Host restart / reconnect causes $0 - \text{CumulativeConsumed}$ unsigned underflow, deadlocking credits. | Implement baseline relative latching on bootstrap `STREAM_CREDIT` frame (`EffectiveConsumed = Reported - Baseline`). |
| **ZO-04** | 🟡 **Major** | Control ACK Starvation | `USB_CMD_RESP_QUEUE` holds both `STREAM_CREDIT` and `CMD_ACK`; high credit frequency silently drops ACKs. | Coalesce credits via `AtomicU16` dirty bitmask; keep `USB_CMD_RESP_QUEUE` dedicated to `CMD_ACK`. |
| **ZO-05** | 🟡 **Major** | Core Codec Scope Gap | `crates/arinc429-core/src/word.rs` lacks `encode_word`, SDI/SSM extraction, and parity application logic. | Add `crates/arinc429-core/src/word.rs` to modified files in implementation plan. |
| **ZO-06** | 🟡 **Major** | Telemetry Eviction for AI | At 16.6k words/s, 32k ring buffer flushes in 11.8s, evicting 1 Hz labels during 10–30s agent turns. | Implement host Current Value Table (CVT) `[12][256]` and expose `pico429_get_latest_labels`. |
| **ZO-07** | 🟡 **Major** | Premature Async Polling | Short streaming tests (< 10k words, < 3.6s) force multi-turn `{job_id}` polling in MCP. | Support synchronous execution by default with `timeout_ms`; reserve `{job_id}` for open-ended runs. |
| **ZO-08** | 🟡 **Major** | Serial Port Lock Contention | `pico429 mcp serve` holds exclusive OS COM handle; external CLI calls fail with `ERROR_ACCESS_DENIED`. | Document concurrency boundary: route CLI operations through MCP or local IPC while server runs. |
| **ZO-09** | 🟢 **Nit** | Wire Protocol Magic Constant | Document specifies `0xA429`, but `protocol.rs` defines `[0x41, 0x29]` (`0x4129`, `"A)"`). | Align document text with `BINARY_MAGIC` in `crates/arinc429-core/src/protocol.rs`. |
| **ZO-10** | 🟢 **Nit** | Dependency Version Pinning | Document pins `rmcp = "0.3"` and separate `rmcp-macros`. `rmcp` is at `3.9.0` with integrated macros. | Update dependency declaration to `rmcp = { version = "3.9", features = ["server", "transport-stdio"] }`. |
| **ZO-11** | 🟢 **Nit** | Micro-Batching Syscall Overhead | Enforcing 64-byte chunks on host serial writes triggers 4,166 `WriteFile` syscalls/sec. | Allow larger host write buffers (e.g. 512 bytes / 32 words); let USB host controller handle packetization. |
| **ZO-12** | 🟢 **Nit** | Global Workspace Profiles | Root `Cargo.toml` sets `opt-level = "s"` globally for firmware. Global `opt-level = 3` bloats firmware flash. | Use package-specific override: `[profile.release.package.pico429] opt-level = 3`. |
