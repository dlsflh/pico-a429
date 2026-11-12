# Goldfish Annotated Review Report

**Target:** `docs/designs/pure_rust_host_and_mcp_server.md`
**Status:** Resolved (All 19 proposed fixes auto-accepted and applied to `docs/designs/pure_rust_host_and_mcp_server.md`; contested item N-03 excluded)
**Date Resolved:** 2026-11-11

## Prioritized Findings & AI Suggestions

### [Issue B-01] Wire Protocol Magic & Status Byte Discrepancy [`egm-design`, `egm-comprehension`] - Severity: Blocker
*   **Affected Scope:** [Lines 84, 128, 142–144](docs/designs/pure_rust_host_and_mcp_server.md#L84-L144) (`Section 1`, `Section 3.1`)
*   **Discrepancy Description:** The specification asserts `Magic (0xA429)` and states command status is in `payload_0`. However, `crates/arinc429-core/src/protocol.rs#L11` defines `BINARY_MAGIC: [u8; 2] = [0x41, 0x29]` (`0x4129`, `"A)"`), and status code is stored in Byte 3 (`flags`). Numeric error codes also conflict with `protocol.rs`.
*   **Rationale:** Packet decoders built against the spec will reject 100% of frames with `InvalidMagic` and extract status from the wrong byte offset.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:** Aligning specification constants and diagrams with authoritative code in `crates/arinc429-core/src/protocol.rs` guarantees protocol consistency across host and firmware.
*   ***Proposed Edit:***
    *   Location: Lines 84, 128, 142–144
    *   Change: Update magic constant to `0x4129` (`BINARY_MAGIC`), update packet diagram to `Magic (0x4129)`, and define `CMD_ACK` status in Byte 3 `flags` matching `arinc429_core::protocol::ack_status`.

---

### [Issue B-02] Control Plane vs Data Plane Queue Decoupling & Queue Purge Opcode [`egm-design`, `egm-zoom-out`] - Severity: Blocker
*   **Affected Scope:** [Lines 198–215, 282–287, 403–418](docs/designs/pure_rust_host_and_mcp_server.md#L198-L215) (`Section 4.1`, `Section 5.1`, `Section 4`)
*   **Discrepancy Description:** The specification claims Lane 0 gives single data words (`pico429_send_word`) `< 1.0 ms` latency by bypassing credit flow control. But in firmware, data words enter `CHANNEL_TX_QUEUES`, a single FIFO where they wait behind 34.56 ms of stream words. Furthermore, uncredited words increment MCU consumption counters, causing host credit underflow and permanent deadlocks. Additionally, stream abort leaves stale words in firmware FIFO without a purge mechanism.
*   **Rationale:** ARINC 429 is a strictly sequential serial waveform. Priority words cannot be spliced into an active PIO stream without corrupting sequence integrity. Control commands (`CMD_REQ`) are processed directly by the M33 core and never enter `CHANNEL_TX_QUEUES`.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:** Define Lane 0 strictly as **Control Plane Only** (`CMD_REQ`: ping, configure, status, abort), which executes in < 1.0 ms in firmware without touching channel FIFOs. Move `pico429_send_word` to Data Plane with normal credit gating (or reject with `ERR_MODE_CONFLICT` if a stream is running on that channel). Define opcode `CMD_REQ: ABORT_TX = 0x0B` in `protocol.rs` and firmware to purge `CHANNEL_TX_QUEUES[ch]` and flush PIO FIFO upon stream cancellation.
*   ***Proposed Edit:***
    *   Location: Section 4.1, Section 5.1, Section 4
    *   Change: Restrict Lane 0 to control frames (`CMD_REQ`). Move `send_word` to Data Plane. Add `ABORT_TX = 0x0B` to opcode definitions and implementation plan.

---

### [Issue B-03] Relative Credit Baseline Latching on Host Bootstrap [`egm-zoom-out`, `egm-inventive-catalyst`] - Severity: Blocker
*   **Affected Scope:** [Lines 220–228, 458](docs/designs/pure_rust_host_and_mcp_server.md#L220-L228) (`Section 4.2.1`, `Section 9`)
*   **Discrepancy Description:** Monotonic tracking $\text{InFlight} = \text{SentCount} - \text{CumulativeConsumed}$ fails when the host restarts (`SentCount = 0`) while firmware has already transmitted words ($\text{CumulativeConsumed} > 0$). $0 - \text{CumulativeConsumed}$ produces an unsigned 32-bit underflow ($4,294,917,296$), permanently locking usable credit to zero.
*   **Rationale:** Under unsigned arithmetic, an uninitialized baseline deadlocks all transmissions on host reconnection.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:** Relative latching on host connection setup synchronizes the session window with zero firmware complexity.
*   ***Proposed Edit:***
    *   Location: Section 4.2.1, Section 9
    *   Change: Specify that upon receiving bootstrap `STREAM_CREDIT`, host latches `BaselineConsumed[ch] = ReportedConsumed[ch]`. Compute: $\text{InFlight}[ch] = \text{SentCount}[ch] - (\text{ReportedConsumed}[ch].\text{wrapping\_sub}(\text{BaselineConsumed}[ch]))$.

---

### [Issue B-04] Hardware Drop Accounting & Zero Credit Leakage [`egm-design`, `egm-zoom-out`] - Severity: Blocker
*   **Affected Scope:** [Lines 220–228, 416](docs/designs/pure_rust_host_and_mcp_server.md#L220-L228) (`Section 4.2.1`, `Section 5`)
*   **Discrepancy Description:** When firmware drops a `STREAM_TX_DATA` frame (e.g. queue full or channel disabled), the word never enters the PIO FIFO and never increments `CumulativeConsumed`. That credit is permanently lost on the host, causing creeping credit exhaustion until the stream stalls.
*   **Rationale:** Credit flow control requires conservation of tokens: every token sent must either be consumed by hardware or accounted as dropped.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:** Track `DROPPED_TX_WORDS: [AtomicU32; ACTIVE_CHANNEL_COUNT]` in firmware and report total handled words (`Consumed + Dropped`) in `STREAM_CREDIT` to ensure host in-flight accounting never drifts.
*   ***Proposed Edit:***
    *   Location: Section 4.2.1, Section 4, Section 5
    *   Change: Firmware increments `DROPPED_TX_WORDS[ch]` when `send_to_channel_tx` fails, and computes cumulative handled words in `STREAM_CREDIT` updates.

---

### [Issue B-05] Package Name and Partitioned Verification Plan [`egm-comprehension`, `egm-implementation-readiness`, `egm-codebase-details`] - Severity: Blocker
*   **Affected Scope:** [Lines 491, 496](docs/designs/pure_rust_host_and_mcp_server.md#L491-L496) (`Verification Plan`)
*   **Discrepancy Description:** `cargo build --package firmware` fails because the package is named `pico-429-firmware` in `crates/firmware/Cargo.toml`. Furthermore, `cargo clippy --workspace --all-targets` fails because bare-metal Cortex-M and host OS targets cannot compile in a single unpartitioned pass.
*   **Rationale:** Verification instructions must be immediately runnable and valid.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:** Target-partitioned verification commands guarantee clean compilation for both embedded and host targets.
*   ***Proposed Edit:***
    *   Location: Lines 491, 496
    *   Change: Replace with:
        `cargo build --package pico-429-firmware --target thumbv8m.main-none-eabihf --release`
        `cargo clippy --package arinc429-core --package pico429 --all-targets -- -D warnings`
        `cargo clippy --package pico-429-firmware --target thumbv8m.main-none-eabihf --all-targets -- -D warnings`

---

### [Issue B-06] Add Missing Codebase Modification Files (`device.rs` and `word.rs`) [`egm-implementation-readiness`, `egm-codebase-details`] - Severity: Blocker
*   **Affected Scope:** [Lines 388–410](docs/designs/pure_rust_host_and_mcp_server.md#L388-L410) (`Target Files to Create/Modify`)
*   **Discrepancy Description:** Transmit words are popped from `CHANNEL_TX_QUEUES` and pushed to PIO in `crates/firmware/src/driver/device.rs`, which was omitted from the plan. Similarly, `crates/arinc429-core/src/word.rs` lacks `encode_word` / `apply_parity` and was omitted from the plan.
*   **Rationale:** Without these files in the plan, consumption counters cannot be triggered in firmware, and word constructors cannot compile on the host.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:** Explicitly enumerating `crates/firmware/src/driver/device.rs` and `crates/arinc429-core/src/word.rs` ensures full implementation readiness.
*   ***Proposed Edit:***
    *   Location: Section 3.1
    *   Change: Add `crates/firmware/src/driver/device.rs` and `crates/arinc429-core/src/word.rs` to Target Files to Create/Modify.

---

### [Issue M-01] Baud-Rate Watermark Scaling & Queue-Empty Tail Flush [`egm-design`, `egm-zoom-out`] - Severity: Major
*   **Affected Scope:** [Lines 229–244, 409](docs/designs/pure_rust_host_and_mcp_server.md#L229-L244) (`Section 4.2.2`, `Section 4.2.3`, `Section 4`)
*   **Discrepancy Description:** A static 32-word consumption threshold with a 32-word ceiling at 12.5 kbps guarantees buffer starvation ($32 - 32 = 0$ runway). Sparse bursts ($< 32$ words) never cross the threshold, permanently leaking credits.
*   **Rationale:** Flow control must dynamically adapt to physical line rate and tail batches.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:** Scale watermark to $\min(32, \text{ceiling}/2)$ (8 words at 12.5k) and trigger credit immediately when `queue.is_empty()`.
*   ***Proposed Edit:***
    *   Location: Section 4.2.2, Section 4.2.3, Section 4
    *   Change: Specify dynamic watermark scaling and queue-empty tail-flush trigger.

---

### [Issue M-02] Atomic Bitmask Credit Coalescing in Firmware [`egm-design`, `egm-zoom-out`] - Severity: Major
*   **Affected Scope:** [Lines 253, 409, 417](docs/designs/pure_rust_host_and_mcp_server.md#L253-L417) (`Section 4.2.4`, `Section 5`)
*   **Discrepancy Description:** Routing discrete 16-byte `STREAM_CREDIT` frames through `USB_CMD_RESP_QUEUE` creates 522 packets/s of USB Full Speed traffic and silently drops `CMD_ACK` frames on queue overflow.
*   **Rationale:** State coalescence via atomic bitmask eliminates queue allocations and prevents ACK drops.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:** An `AtomicU16` dirty bitmask allows `usb_cdc_tx_task` to pack credit updates directly into 64-byte batch transfers without queue contention.
*   ***Proposed Edit:***
    *   Location: Section 4.2.4, Section 5
    *   Change: Replace credit queuing in `USB_CMD_RESP_QUEUE` with `CREDIT_DIRTY_MASK: AtomicU16`.

---

### [Issue M-03] Firmware USB CDC Write Timeout Expansion [`egm-design`] - Severity: Major
*   **Affected Scope:** [Lines 251, 358](docs/designs/pure_rust_host_and_mcp_server.md#L251-L358) (`Section 4.2.4`, `NFR`)
*   **Discrepancy Description:** Hardcoded 100 ms write timeout in `crates/firmware/src/usb_service.rs#L177` tears down the CDC connection before the 245 ms buffer fills during host scheduling stalls.
*   **Rationale:** The write timeout must exceed the hardware buffer absorption time.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:** Increasing timeout to $\ge 300\text{ ms}$ preserves connection state during host OS scheduling jitter.
*   ***Proposed Edit:***
    *   Location: Section 4.2.4, Section 5, NFR
    *   Change: Update firmware CDC write timeout to 300 ms.

---

### [Issue M-04] Bootstrap Initial Credit Broadcast in Firmware [`egm-implementation-readiness`, `egm-codebase-details`] - Severity: Major
*   **Affected Scope:** [Lines 227, 411–418](docs/designs/pure_rust_host_and_mcp_server.md#L227-L418) (`Section 4.2.1`, `Section 5`)
*   **Discrepancy Description:** Specification requires initial credits upon USB connection, but the implementation plan omitted this logic in `usb_service.rs`, leaving host streaming paused with credit 0.
*   **Rationale:** Host cannot transmit stream data without bootstrap credits.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:** Add connection bootstrap loop in `usb_service.rs` emitting initial `STREAM_CREDIT` for active TX channels.
*   ***Proposed Edit:***
    *   Location: Section 5
    *   Change: In `usb_cdc_tx_task`, emit initial credit frame upon host enumeration.

---

### [Issue M-05] Host Current Value Table (CVT) `[12][256]` for AI Agents [`egm-zoom-out`] - Severity: Major
*   **Affected Scope:** [Lines 301, 359, 463](docs/designs/pure_rust_host_and_mcp_server.md#L301-L463) (`Section 5.2`, `NFR`, `Section 10`)
*   **Discrepancy Description:** 16.6k words/s telemetry flushes sequential 32k ring buffers in ~2 seconds, while AI agent turns take 10–20 seconds.
*   **Rationale:** AI agents require $O(1)$ deterministic access to the latest value of any ARINC label regardless of turn latency.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:** A host-side table of `[AtomicU32; 256]` per channel provides instantaneous state lookup with minimal memory footprint (12 KB).
*   ***Proposed Edit:***
    *   Location: Section 5.2, Section 10
    *   Change: Add `crates/pico429/src/telemetry/cvt.rs` and expose MCP tool `pico429_get_latest_labels`.

---

### [Issue M-06] Stream Scheduler Concurrency Topology [`egm-implementation-readiness`] - Severity: Major
*   **Affected Scope:** [Lines 452–459](docs/designs/pure_rust_host_and_mcp_server.md#L452-L459) (`Section 9`)
*   **Discrepancy Description:** A single shared Lane 1 MPSC channel causes backpressured channels to block words destined for unblocked channels.
*   **Rationale:** Multi-channel streaming requires per-channel flow-control independence.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:** Use per-channel asynchronous credit semaphores or 12 dedicated MPSC channels with round-robin polling in `crates/pico429/src/transport/scheduler.rs`.
*   ***Proposed Edit:***
    *   Location: Section 9
    *   Change: Specify per-channel semaphores in `scheduler.rs`.

---

### [Issue M-07] Synchronous Default for MCP Streaming & Job Manager Architecture [`egm-zoom-out`, `egm-implementation-readiness`] - Severity: Major
*   **Affected Scope:** [Lines 54, 298–300, 467–473](docs/designs/pure_rust_host_and_mcp_server.md#L54-L473) (`Open Questions`, `Section 5.2`, `Section 11`)
*   **Discrepancy Description:** Requiring multi-turn `{job_id}` polling for short tests (< 10k words, < 3.6s) wastes tokens and increases turn latency.
*   **Rationale:** Short streaming operations should execute synchronously by default.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:** Support synchronous execution with `timeout_ms` by default; add `tokio-util` and `uuid` to dependencies for unbounded background jobs.
*   ***Proposed Edit:***
    *   Location: Section 5.2, Section 11, Open Questions
    *   Change: Add synchronous mode and specify `StreamJobManager`.

---

### [Issue M-08] Windows High-Resolution Timer Initialization [`egm-implementation-readiness`] - Severity: Major
*   **Affected Scope:** [Lines 444–451](docs/designs/pure_rust_host_and_mcp_server.md#L444-L451) (`Section 8`)
*   **Discrepancy Description:** Default Windows timer resolution (15.6 ms) throttles 250 µs batch timeouts by $60\times$, collapsing USB streaming throughput.
*   **Rationale:** Sub-millisecond timers require explicit OS multimedia timer configuration on Windows.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:** Mandate calling `timeBeginPeriod(1)` or using an adaptive hybrid wait in host startup.
*   ***Proposed Edit:***
    *   Location: Section 8
    *   Change: Specify `timeBeginPeriod(1)` on Windows in `crates/pico429/src/main.rs`.

---

### [Issue M-09] Host Library Exposure for Integration Tests (`src/lib.rs`) [`egm-implementation-readiness`] - Severity: Major
*   **Affected Scope:** [Lines 419–443, 494](docs/designs/pure_rust_host_and_mcp_server.md#L419-L494) (`Section 6`, `Section 7`, `Verification Plan`)
*   **Discrepancy Description:** Declaring modules solely in `main.rs` prevents integration tests in `crates/pico429/tests/` from compiling.
*   **Rationale:** Integration test suites require library crate exposure.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:** Add `crates/pico429/src/lib.rs` and specify integration test files in target files list.
*   ***Proposed Edit:***
    *   Location: Section 6, Section 7, Target Files
    *   Change: Add `crates/pico429/src/lib.rs` to Target Files.

---

### [Issue M-10] Document Architecture & Layout Symmetry Fixes [`egm-structural`, `egm-style`] - Severity: Major
*   **Affected Scope:** [Lines 125–138, 201–237, 301, 361–376](docs/designs/pure_rust_host_and_mcp_server.md#L125-L376)
*   **Discrepancy Description:** Appendixes are misnested as H3 subheadings under NFR; headings contain Chinese parentheticals; bit diagram columns are misaligned (10/6 chars vs 8/8); `<u8>` in table is unescaped; Section 3 has orphaned `3.1`.
*   **Rationale:** Preserves document hierarchy and clean markdown rendering.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:** Clean structural layout ensures template compliance.
*   ***Proposed Edit:***
    *   Location: Lines 125–138, 201–237, 301, 361–376
    *   Change: Promote appendixes to H2; remove Chinese parentheticals from headings; realign diagram; escape `<u8>`.

---

### [Issue M-11] Legacy Python File Paths & Profile Overrides [`egm-codebase-details`, `egm-style`] - Severity: Major
*   **Affected Scope:** [Lines 386, 477–478](docs/designs/pure_rust_host_and_mcp_server.md#L386-L478)
*   **Discrepancy Description:** Target list references non-existent Python paths (`tools/pico429/pico429_cli.py`); global `opt-level = 3` bloats firmware flash.
*   **Rationale:** Accurate repository paths and correct profile overrides are required.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:** Point to actual files `tools/pico429.py` and `tools/arinc429_codec.py`. Add package profile override `[profile.release.package.pico429] opt-level = 3`.
*   ***Proposed Edit:***
    *   Location: Lines 386, 477–478
    *   Change: Update target paths and package profile override syntax.

---

### [Issue N-01] Style Calibration & Metric Harmonization [`egm-style`] - Severity: Nit
*   **Affected Scope:** [Lines 11, 31, 217, 351, 354](docs/designs/pure_rust_host_and_mcp_server.md)
*   **Discrepancy Description:** Ungrounded superlative "absolute immunity"; CPU utilization targets fluctuate between `< 0.2%` and `< 0.5%`.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:** Harmonize CPU target to `< 0.5% CPU load` and replace "absolute immunity" with "bounded jitter tolerance up to 34.56 ms".
*   ***Proposed Edit:***
    *   Location: Lines 11, 217, 354
    *   Change: Align phrasing and metric numbers.

---

### [Issue N-02] Dependency Features Pinning (`rmcp`) [`egm-implementation-readiness`, `egm-codebase-details`] - Severity: Nit
*   **Affected Scope:** [Lines 427–428](docs/designs/pure_rust_host_and_mcp_server.md#L427-L428)
*   **Discrepancy Description:** `rmcp` SDK version is `3.0` and requires `transport-io` for stdio transport; separate `rmcp-macros` is redundant.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:** Use `rmcp = { version = "3.0", features = ["server", "macros", "transport-io"] }`.
*   ***Proposed Edit:***
    *   Location: Line 427
    *   Change: Update `rmcp` dependency line.

---

### [Issue N-03] Proposed Firmware 1-Word `EXPEDITED_TX_SLOT` in Inner Loop [`egm-zoom-out` Stage 8 Review] - Severity: Blocker (Contested)
*   **Affected Scope:** [Findings ZO-01 and CR-02 Recommendation]
*   **Discrepancy Description:** ZO-01 and CR-02 originally proposed adding an `EXPEDITED_TX_SLOT: [AtomicU32; 12]` in firmware to allow Lane 0 data words to cut in front of `CHANNEL_TX_QUEUES`.
*   ***AI Suggestion:*** CONTEST
*   ***Justification:** As evaluated in Stage 8, adding an out-of-band expedited slot to the hardware PIO feeder loop is an over-engineering anti-pattern (YAGNI violation) and physically corrupts sequential ARINC 429 bitstreams. Control commands (`CMD_REQ`) are processed directly by M33 in `usb_service.rs` and never enter channel FIFOs anyway. Single-word test transmissions belong to the Data Plane with standard credit gating. Immediate stream cancellation is cleanly solved via the `CMD_REQ: ABORT_TX` opcode without adding branching overhead to the PIO loop.
*   ***Proposed Edit:***
    *   Location: N/A (Contested recommendation; do not add firmware slot).
