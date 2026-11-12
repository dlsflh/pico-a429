# Stage 7 Report: Antagonistic Critic Review

*   **Target Document**: `docs/designs/pure_rust_host_and_mcp_server.md`
*   **Target Range**: FeatureRange (lines 1..377)
*   **Reviewers**: `egm-design` (Hostile Design Critic), `egm-structural` (Structural Layout Validator), `egm-style` (Technical Copy Editor)
*   **Status**: Complete

---

## Executive Summary

The antagonistic review of the feature specification uncovered **three critical architectural blockers**, **two technical copy blockers**, and **eight major structural and system-level contradictions**.

Most notably:
1. **Credit Tracking Deadlock & Panic**: Injecting priority words via Lane 0 bypasses host credit tracking but increments firmware's `cumulative_consumed_counter`. On the host, $\text{SentCount} < \text{CumulativeConsumed}$ triggers unsigned integer underflow panic in debug mode and permanently locks the stream with credit `0` in release mode.
2. **Magic Constant Incompatibility**: The specification asserts `0xA429`, but the authoritative core crate `crates/arinc429-core` defines `BINARY_MAGIC = [0x41, 0x29]` (`0x4129`).
3. **Credit Leakage on Hardware Drops**: Firmware silently drops `STREAM_TX_DATA` if full or unconfigured; because the word never enters the queue, it is never consumed, permanently leaking credits on the host.
4. **12.5 kbps Refill Starvation**: A fixed 32-word refill watermark with a 32-word credit ceiling at 12.5 kbps guarantees complete buffer starvation ($32 - 32 = 0$ runway) and deadlocks sparse streams (< 32 words).
5. **Firmware USB CDC 100 ms Timeout**: Hardcoded 100 ms write timeout in `usb_service.rs` tears down the CDC connection, voiding the claimed 245.7 ms telemetry absorption window.

---

## Findings Matrix

| ID | Priority | Dimension | Core Finding | Actionable Remediation |
| :--- | :--- | :--- | :--- | :--- |
| **CR-01** | 🔴 **Blocker** | Protocol / Contract | Specification asserts `Magic (0xA429)`, but `arinc429-core` defines `0x4129` (`"A)"`). | Align document to `0x4129` (`BINARY_MAGIC: [u8; 2] = [0x41, 0x29]`). |
| **CR-02** | 🔴 **Blocker** | Queue / Credit Math | Lane 0 words bypass host credit tracking but increment MCU consumed count, causing underflow panic and deadlocking credits. | Allocate dedicated 1-word `EXPEDITED_TX_SLOT` in firmware, or force Lane 0 to participate in monotonic credit accounting. |
| **CR-03** | 🔴 **Blocker** | Flow Control Integrity | Dropped `STREAM_TX_DATA` frames in firmware never reach PIO and never increment consumed count, causing irreversible credit leakage. | Track `dropped_tx_words_counter` in firmware and report via `STREAM_CREDIT` to reconcile host in-flight count. |
| **CR-04** | 🔴 **Blocker** | Style / Precision | Ungrounded claim of "absolute immunity" to OS latency jitter on non-real-time Windows. | Calibrate phrasing to: "bounded jitter tolerance up to 34.56 ms". |
| **CR-05** | 🔴 **Blocker** | Style / Metric Drift | Overview specifies `< 0.2% CPU utilization` while Goals, CUJ 1, and NFR specify `< 0.5%`. | Harmonize all CPU metrics across document to `< 0.5% CPU load`. |
| **CR-06** | 🟡 **Major** | Low-Speed Starvation | Fixed 32-word refill watermark with 32-word ceiling at 12.5 kbps leaves 0 ms runway; sparse streams (< 32 words) never trigger credit. | Scale watermark to 8 words drained at 12.5k; add 5–10 ms empty-queue tail-flush timer in firmware. |
| **CR-07** | 🟡 **Major** | Firmware Timeout Limit | Hardcoded 100 ms write timeout in `usb_service.rs` tears down connection before 245 ms buffer fills. | Increase CDC write timeout to $\ge 300\text{ ms}$ in `usb_service.rs`. |
| **CR-08** | 🟡 **Major** | USB Short-Packet Flood | Dispatching 521 isolated 16-byte `STREAM_CREDIT` frames/s consumes over 50% of USB Full Speed 1 ms transaction slots. | Coalesce credit updates via atomic dirty bitmask directly into outgoing USB batch buffers. |
| **CR-09** | 🟡 **Major** | Hardware Configuration | Software dynamic direction reconfig (`configure_channels`) conflicts with fixed jumper/DIP switch routing on Pico 2 W. | Add hardware board profile check rejecting conflicting direction changes with `ERR_MODE_CONFLICT`. |
| **CR-10** | 🟡 **Major** | Document Structure | `Additional Resources` and `Future Improvements` misnested as H3 subheadings under `Non-Functional Requirements`. | Promote to top-level H2 sections preceding Detailed Implementation Plan. |
| **CR-11** | 🟡 **Major** | Diagram Bit Alignment | Wire diagram allocates 10 chars (5 bits) to `FrameType` and 6 chars (3 bits) to `Chan` instead of 8/8 chars (4/4 bits). | Realign RFC diagram columns so both nibbles occupy exactly 8 characters. |
| **CR-12** | 🟡 **Major** | Markdown Table Syntax | `Option<u8>` in table written without backticks, interpreted as unclosed HTML tag `<u8>` and suppressed in rendering. | Wrap in code backticks: `` `label_filter`: Option<u8> ``. |
| **CR-13** | 🟡 **Major** | Structural Symmetry | Section 3 contains orphaned `#### 3.1` subsection without any corresponding `3.2`. | Introduce `#### 3.1 Binary Frame Layout` and `#### 3.2 Frame Types Definition`. |
| **CR-14** | 🟡 **Major** | Language Consistency | Headings contain sporadic colloquial Chinese parentheticals (`提前补水机制与防断流跑道`, `分级治理`). | Remove non-English parentheticals from headings in English technical document. |
| **CR-15** | 🟢 **Nit** | Hardware Initialization | Pico 2 W leaves CYW43439 power pin (`WL_ON`, GPIO 23) floating, causing 10–50 mA power leakage. | Document driving GPIO 23 low in firmware initialization to power down wireless module. |
| **CR-16** | 🟢 **Nit** | MCP Input Validation | `pico429_stream_send` missing physical line-rate validation for `rate_hz` against configured baud. | Validate `rate_hz \le 2778` (100k) or `\le 347` (12.5k). |
| **CR-17** | 🟢 **Nit** | Formatting / Style | Box boundary in Section 4 ASCII diagram has unclosed parenthesis `(128w/ch|`. | Reformat box text to close parenthesis within border margins. |
| **CR-18** | 🟢 **Nit** | Header Metadata | Document header missing `Target Path` metadata line. | Add `* **Target Path**: docs/designs/pure_rust_host_and_mcp_server.md`. |
