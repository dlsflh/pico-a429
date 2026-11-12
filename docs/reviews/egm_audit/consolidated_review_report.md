# Consolidated EGM Review Report: Pure Rust Host & Native MCP Server

*   **Target Document**: [`docs/designs/pure_rust_host_and_mcp_server.md`](file:///c:/Users/lihan/Work/pico-429/docs/designs/pure_rust_host_and_mcp_server.md) (Lines 1..506)
*   **Protocol Executed**: Elephant-Goldfish Model (EGM) Full Review Protocol (Stages 5–9)
*   **Status**: Final Review Report (Ready for Remediation / Human Decision Gate)

---

## 1. Executive Summary

A comprehensive multi-agent audit was conducted across five independent review stages (Initial Zoom Out, Comprehension Test, Critic Review, Intermediate Zoom Out, and Implementation Readiness), engaging nine specialized Goldfish reviewer personas.

### The Verdict: Sound Architecture, Critical Physical Boundaries Require Remediation
The strategic architectural decision to unify the host tooling as a pure Rust binary (`crates/pico429`) within the Cargo workspace—sharing [`crates/arinc429-core`](file:///c:/Users/lihan/Work/pico-429/crates/arinc429-core/src/protocol.rs)—is affirmed as optimal. It completely eliminates dual-language opcode drift, achieves zero-allocation hot-path throughput, and natively embeds the official `rmcp` Model Context Protocol SDK.

However, the detailed execution and protocol specifications contain **six critical blockers** and **eleven major architectural discrepancies** that would cause compile-time failures, frame synchronization rejection, and flow-control deadlocks if implemented without correction.

---

## 2. Consolidated Actionable Tasks by Priority

### 🔴 Blockers (Must Be Fixed Before Implementation Begins)

*   **TASK-B01 [Protocol & Contract Alignment]**:
    *   *Finding*: The specification cites `Magic (0xA429)`, but [`crates/arinc429-core/src/protocol.rs`](file:///c:/Users/lihan/Work/pico-429/crates/arinc429-core/src/protocol.rs#L11) defines `BINARY_MAGIC: [u8; 2] = [0x41, 0x29]` (`0x4129`, `"A)"`). Furthermore, the specification incorrectly places the `CMD_ACK` status in `payload_0` instead of Byte 3 (`flags`), and lists contradictory error codes.
    *   *Action*: Synchronize specification text, diagram, and tables with `protocol.rs` (`0x4129` and Byte 3 `flags` status mapping).
    *   *Traceability*: ZO-09, C-01, C-02, CR-01, IZO-06.

*   **TASK-B02 [Control Plane vs Data Plane Decoupling & Queue Purge Opcode]**:
    *   *Finding*: The specification claims Lane 0 provides `< 1.0 ms` latency for single data words (`pico429_send_word`) by bypassing credit flow control. However, in firmware, data words enter `CHANNEL_TX_QUEUES`, a single FIFO where they wait behind 34.56 ms of stream words and increment consumption counters uncredited, causing host credit underflow and deadlocks.
    *   *Action*: Restrict Lane 0 strictly to **Control Plane Only** (`CMD_REQ`: status, configure, ping, abort). Control commands are processed immediately in `execute_binary_command` and never enter channel FIFOs. Move `pico429_send_word` to Data Plane with normal credit gating. Define opcode `CMD_REQ: ABORT_TX = 0x0B` in `protocol.rs` and implement queue purging in `channel.rs` and `device.rs` to flush stale words upon stream cancellation.
    *   *Traceability*: ZO-01, CR-02, IZO-01.

*   **TASK-B03 [Relative Credit Baseline Latching on Host Bootstrap]**:
    *   *Finding*: Monotonic tracking $\text{InFlight} = \text{SentCount} - \text{CumulativeConsumed}$ causes unsigned 32-bit underflow ($0 - 50,000 = 4,294,917,296$) and permanent credit lockout when the host restarts or reconnects while firmware has already transmitted words.
    *   *Action*: On initial bootstrap `STREAM_CREDIT`, latch `BaselineConsumed[ch] = ReportedConsumed[ch]`. Compute:
        $$\text{InFlight}[ch] = \text{SentCount}[ch] - (\text{ReportedConsumed}[ch].\text{wrapping\_sub}(\text{BaselineConsumed}[ch]))$$
    *   *Traceability*: ZO-03, IZO-03.

*   **TASK-B04 [Hardware Drop Accounting & Zero Credit Leakage]**:
    *   *Finding*: When firmware drops a `STREAM_TX_DATA` frame (e.g. queue full or channel disabled), the word never enters the PIO FIFO and never increments `CumulativeConsumed`. That credit is permanently lost on the host, producing creeping credit starvation.
    *   *Action*: Track `DROPPED_TX_WORDS: [AtomicU32; ACTIVE_CHANNEL_COUNT]` in firmware and report it in `STREAM_CREDIT` (or count in cumulative handled words) so the host accurately reconciles in-flight accounting.
    *   *Traceability*: CR-03, IZO-04.

*   **TASK-B05 [Correct Verification Commands & Target-Partitioned Clippy]**:
    *   *Finding*: The verification plan instructs `cargo build --package firmware`, which fails because the actual package name is `pico-429-firmware`. Furthermore, `cargo clippy --workspace --all-targets` fails because bare-metal Cortex-M and host OS targets cannot compile in a single unpartitioned pass.
    *   *Action*: Update package name to `pico-429-firmware`. Partition clippy into host (`--package arinc429-core --package pico429`) and bare-metal (`--package pico-429-firmware --target thumbv8m.main-none-eabihf`).
    *   *Traceability*: C-04, IR-01, IZO-12.

*   **TASK-B06 [Add Missing Target Files to Implementation Plan (`device.rs` and `word.rs`)]**:
    *   *Finding*: Transmit words are popped from `CHANNEL_TX_QUEUES` and pushed to PIO in [`crates/firmware/src/driver/device.rs`](file:///c:/Users/lihan/Work/pico-429/crates/firmware/src/driver/device.rs), which was omitted from the plan. Similarly, [`crates/arinc429-core/src/word.rs`](file:///c:/Users/lihan/Work/pico-429/crates/arinc429-core/src/word.rs) lacks `encode_word` / `apply_parity` and was omitted from the plan.
    *   *Action*: Add both files to the target files list with explicit implementation tasks.
    *   *Traceability*: ZO-05, C-03, IR-02, IR-03.

---

### 🟡 Majors (System Integrity, Performance & Usability Architecture)

*   **TASK-M01 [Baud-Rate Watermark Scaling & Queue-Empty Tail Flush]**:
    *   *Finding*: A static 32-word consumption threshold with a 32-word ceiling at 12.5 kbps guarantees buffer starvation ($32 - 32 = 0$ runway). Sparse bursts ($< 32$ words) never cross the threshold, permanently leaking credits.
    *   *Action*: Trigger credit when consumed $\ge \min(32, \text{ceiling}/2)$ (8 words at 12.5k, 32 words at 100k) **OR** when `queue.is_empty()`.
    *   *Traceability*: C-05, CR-06, IZO-05.

*   **TASK-M02 [Atomic Bitmask Credit Coalescing in Firmware]**:
    *   *Finding*: Routing discrete 16-byte `STREAM_CREDIT` frames through `USB_CMD_RESP_QUEUE` creates 522 packets/s of USB Full Speed traffic and silently drops `CMD_ACK` frames on queue overflow.
    *   *Action*: Use `AtomicU16` dirty bitmask to coalesce credits directly into outgoing USB batch buffers; reserve `USB_CMD_RESP_QUEUE` strictly for lossless `CMD_ACK` frames.
    *   *Traceability*: ZO-04, CR-08, IZO-08.

*   **TASK-M03 [Firmware USB CDC Write Timeout Expansion]**:
    *   *Finding*: Hardcoded 100 ms write timeout in `crates/firmware/src/usb_service.rs#L177` tears down the CDC connection before the 245 ms buffer fills during host scheduling stalls.
    *   *Action*: Increase CDC write timeout in `usb_service.rs` to $\ge 300\text{ ms}$.
    *   *Traceability*: CR-07, IZO-07.

*   **TASK-M04 [Bootstrap Initial Credit Broadcast in Firmware]**:
    *   *Finding*: Specification requires initial credits upon USB connection, but the implementation plan omitted this logic in `usb_service.rs`, leaving host streaming paused with credit 0.
    *   *Action*: Enqueue initial `STREAM_CREDIT` frames for all active TX channels upon host CDC enumeration / DTR assertion.
    *   *Traceability*: IR-04, IZO-03.

*   **TASK-M05 [Host Current Value Table (CVT) `[12][256]` for AI Agents]**:
    *   *Finding*: 16.6k words/s telemetry flushes sequential 32k ring buffers in ~2 seconds, while AI agent turns take 10–20 seconds.
    *   *Action*: Maintain host-side `[AtomicU32; 256]` array per channel tracking latest received word of every label. Expose MCP tool `pico429_get_latest_labels`.
    *   *Traceability*: ZO-06, IZO-10.

*   **TASK-M06 [Stream Scheduler Concurrency Topology]**:
    *   *Finding*: A single shared Lane 1 MPSC channel causes backpressured channels to block words destined for unblocked channels.
    *   *Action*: Use per-channel credit semaphores or 12 dedicated MPSC channels with round-robin polling in `crates/pico429/src/transport/scheduler.rs`.
    *   *Traceability*: IR-05.

*   **TASK-M07 [Synchronous Default for MCP Streaming & Job Manager Architecture]**:
    *   *Finding*: Requiring multi-turn `{job_id}` polling for short tests (< 10k words, < 3.6s) wastes tokens and increases turn latency.
    *   *Action*: Support synchronous execution by default with `timeout_ms`; add `tokio-util` and `uuid` to dependencies, and specify `StreamJobManager` for open-ended runs.
    *   *Traceability*: ZO-07, IR-08, IZO-11.

*   **TASK-M08 [Windows High-Resolution Timer Initialization]**:
    *   *Finding*: Default Windows timer resolution (15.6 ms) throttles 250 µs batch timeouts by $60\times$, collapsing USB streaming throughput.
    *   *Action*: Mandate calling `timeBeginPeriod(1)` or using an adaptive hybrid wait in host startup.
    *   *Traceability*: IR-07, IZO-18.

*   **TASK-M09 [Host Library Exposure for Integration Tests (`src/lib.rs`)]**:
    *   *Finding*: Declaring modules solely in `main.rs` prevents integration tests in `crates/pico429/tests/` from compiling.
    *   *Action*: Add `crates/pico429/src/lib.rs` and specify integration test files in target files list.
    *   *Traceability*: IR-06.

*   **TASK-M10 [Document Architecture & Layout Symmetry Fixes]**:
    *   *Finding*: `Additional Resources` and `Future Improvements` are misnested as H3 subheadings under `Non-Functional Requirements`; headings contain Chinese parentheticals; bit diagram columns are misaligned; `<u8>` in table is unescaped.
    *   *Action*: Promote appendixes to top-level H2 sections; remove non-English parentheticals from headings; realign diagram (8 chars per nibble); escape table angle brackets.
    *   *Traceability*: CR-10, CR-11, CR-12, CR-13, CR-14, IZO-15.

*   **TASK-M11 [Legacy File References & Profile Overrides]**:
    *   *Finding*: Target list references non-existent Python paths (`tools/pico429/pico429_cli.py`); global `opt-level = 3` bloats firmware flash.
    *   *Action*: Correct paths to `tools/pico429.py` and `tools/arinc429_codec.py`. Add package profile override `[profile.release.package.pico429] opt-level = 3`.
    *   *Traceability*: IR-09, IR-10, IZO-12.

---

### 🟢 Nits (Code Quality, Tooling & Precision Refinements)

*   **TASK-N01 [Style Calibration]**: Replace ungrounded superlative "absolute immunity" with "bounded jitter tolerance up to 34.56 ms"; harmonize CPU utilization target to `< 0.5% CPU load` across all sections. (CR-04, CR-05, IZO-14)
*   **TASK-N02 [Dependency Pinning]**: Pin `rmcp = { version = "3.0", features = ["server", "macros", "transport-io"] }` and drop redundant `rmcp-macros`. (IR-11, IZO-16)
*   **TASK-N03 [Hardware Low-Power Initialization]**: Drive GPIO 23 (`WL_ON`) low during firmware startup on Pico 2 W to eliminate floating power leakage on the wireless module. (CR-15, IZO-17)
*   **TASK-N04 [Host Syscall Optimization]**: Allow host serial write buffering up to 512 bytes rather than enforcing strict 64-byte userland chunks, reducing OS `WriteFile` syscall frequency by 8x. (IZO-18)
*   **TASK-N05 [Direct Library Reuse]**: Directly reuse existing `arinc429_core::protocol::find_frame_in_stream` in host transport. (IR-12)

---

## 3. Traceability Matrix

| Raw Finding ID | Stage & Reviewer | Consolidated Task | Target File(s) |
| :--- | :--- | :--- | :--- |
| **ZO-01, CR-02, IZO-01** | Stage 5/7/8 (Architect, Critic) | **TASK-B02** | `crates/arinc429-core/src/protocol.rs`, `crates/firmware/src/channel.rs` |
| **ZO-02** | Stage 5 (Architect) | **TASK-M10** | `docs/designs/pure_rust_host_and_mcp_server.md` |
| **ZO-03, IZO-03** | Stage 5/8 (Architect) | **TASK-B03** | `crates/pico429/src/transport/scheduler.rs` |
| **ZO-04, CR-08, IZO-08** | Stage 5/7/8 (Architect, Critic) | **TASK-M02** | `crates/firmware/src/usb_service.rs` |
| **ZO-05, C-03, IR-03** | Stage 5/6/9 (Architect, Reader, Lead) | **TASK-B06** | `crates/arinc429-core/src/word.rs` |
| **ZO-06, IZO-10** | Stage 5/8 (Architect) | **TASK-M05** | `crates/pico429/src/telemetry/cvt.rs`, `crates/pico429/src/mcp/server.rs` |
| **ZO-07, IR-08, IZO-11** | Stage 5/8/9 (Architect, Lead) | **TASK-M07** | `crates/pico429/src/mcp/server.rs`, `Cargo.toml` |
| **ZO-08** | Stage 5 (Architect) | **TASK-M10** | `docs/designs/pure_rust_host_and_mcp_server.md` |
| **ZO-09, C-01, C-02, CR-01, IZO-06** | Stage 5/6/7/8 (All Reviewers) | **TASK-B01** | `crates/arinc429-core/src/protocol.rs`, `docs/designs/...` |
| **ZO-10, IR-11, IZO-16** | Stage 5/8/9 (Architect, Lead) | **TASK-N02** | `crates/pico429/Cargo.toml` |
| **ZO-11, IZO-18** | Stage 5/8 (Architect) | **TASK-N04** | `crates/pico429/src/transport/device.rs` |
| **ZO-12, IR-10, IZO-12** | Stage 5/8/9 (Architect, Details) | **TASK-M11** | `Cargo.toml` |
| **C-04, IR-01** | Stage 6/9 (Reader, Lead, Details) | **TASK-B05** | `docs/designs/pure_rust_host_and_mcp_server.md` |
| **C-05, CR-06, IZO-05** | Stage 6/7/8 (Reader, Critic, Architect) | **TASK-M01** | `crates/firmware/src/channel.rs`, `crates/firmware/src/usb_service.rs` |
| **C-06** | Stage 6 (Reader) | **TASK-M07** | `crates/pico429/src/main.rs` |
| **C-07** | Stage 6 (Reader) | **TASK-B01** | `docs/designs/pure_rust_host_and_mcp_server.md` |
| **C-08, CR-09** | Stage 6/7 (Reader, Critic) | **TASK-M10** | `docs/designs/pure_rust_host_and_mcp_server.md` |
| **CR-03, IZO-04** | Stage 7/8 (Critic, Architect) | **TASK-B04** | `crates/firmware/src/channel.rs`, `crates/pico429/src/transport/scheduler.rs` |
| **CR-04, CR-05, IZO-14** | Stage 7/8 (Style, Architect) | **TASK-N01** | `docs/designs/pure_rust_host_and_mcp_server.md` |
| **CR-07, IZO-07** | Stage 7/8 (Critic, Architect) | **TASK-M03** | `crates/firmware/src/usb_service.rs` |
| **CR-10..14, IZO-15** | Stage 7/8 (Structural, Style) | **TASK-M10** | `docs/designs/pure_rust_host_and_mcp_server.md` |
| **CR-15, IZO-17** | Stage 7/8 (Critic, Architect) | **TASK-N03** | `crates/firmware/src/main.rs` |
| **IR-02** | Stage 9 (Lead, Details) | **TASK-B06** | `crates/firmware/src/driver/device.rs` |
| **IR-04** | Stage 9 (Lead, Details) | **TASK-M04** | `crates/firmware/src/usb_service.rs` |
| **IR-05** | Stage 9 (Lead) | **TASK-M06** | `crates/pico429/src/transport/scheduler.rs` |
| **IR-06** | Stage 9 (Lead) | **TASK-M09** | `crates/pico429/src/lib.rs` |
| **IR-07** | Stage 9 (Lead) | **TASK-M08** | `crates/pico429/src/main.rs` |
| **IR-09** | Stage 9 (Lead, Details) | **TASK-M11** | `docs/designs/pure_rust_host_and_mcp_server.md`, `tools/` |
| **IR-12** | Stage 9 (Details) | **TASK-N05** | `crates/pico429/src/transport/device.rs` |

---

## 4. Next Steps & Recommendation

The review pipeline is complete. All 5 stage reports are compiled on disk:
- [`zoom_out_report.md`](file:///c:/Users/lihan/Work/pico-429/zoom_out_report.md) (Stage 5)
- [`comprehension_report.md`](file:///c:/Users/lihan/Work/pico-429/comprehension_report.md) (Stage 6)
- [`critic_report.md`](file:///c:/Users/lihan/Work/pico-429/critic_report.md) (Stage 7)
- [`intermediate_zoom_out_report.md`](file:///c:/Users/lihan/Work/pico-429/intermediate_zoom_out_report.md) (Stage 8)
- [`implementation_readiness_report.md`](file:///c:/Users/lihan/Work/pico-429/implementation_readiness_report.md) (Stage 9)

**Recommended Action**: Proceed to execute automated remediation (`egm-resolve`) to apply the consolidated tasks (TASK-B01 through TASK-B06, and TASK-M01 through TASK-M11) directly to [`docs/designs/pure_rust_host_and_mcp_server.md`](file:///c:/Users/lihan/Work/pico-429/docs/designs/pure_rust_host_and_mcp_server.md) so the specification is 100% hardened, correct, and ready for code implementation.
