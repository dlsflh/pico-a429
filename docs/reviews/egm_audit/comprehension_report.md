# Stage 6 Report: Comprehension Test

*   **Target Document**: `docs/designs/pure_rust_host_and_mcp_server.md` (lines 1..506)
*   **Reviewer**: `egm-comprehension` (Naive Reader)
*   **Status**: Complete

---

## Feature Specification Paraphrase

### What this proposes
The document proposes replacing the legacy Python host tooling for the Pico-429 multi-channel ARINC 429 transceiver with a single, standalone Rust binary crate (`crates/pico429`). It directly imports the shared `crates/arinc429-core` protocol library, eliminating dual-language maintenance and protocol drift with the RP2350 firmware. The architecture introduces an asynchronous dual-lane streaming engine over USB CDC capable of sustaining 533.3 KB/s (33,334 words/s across 12 channels) with sub-millisecond control latency and cumulative sliding-window flow control. Additionally, it embeds an official Model Context Protocol (MCP) server directly into the binary over stdio, providing type-safe, schema-validated tools for AI coding agents to control, stream, and diagnose ARINC 429 hardware without subshell overhead.

### How the existing system works, per this document
The existing host software consists of Python prototype scripts located in `tools/pico429/` that communicate with an RP2350A (Raspberry Pi Pico 2 W) microcontroller running embedded Rust firmware. The system suffers from a language fracture where protocol framing, constants, and CRC algorithms must be manually maintained in both Rust (`arinc429-core`) and Python (`arinc429_codec.py`), causing opcode drift. Transmission uses a synchronous stop-and-wait `CMD_REQ`/`CMD_ACK` round-trip per word, which is throttled by Windows USB CDC scheduling latency to 1,000–2,000 words/s across all channels, starving high-speed 100 kbps ARINC 429 transmitters. Furthermore, CPython runtime overhead, garbage collection pauses, and GIL contention consume up to 100% of a CPU core during streaming, and AI agents must rely on brittle subshell script invocations and unvalidated text scraping.

---

## Implementation Plan Paraphrase

The implementation plan registers a new host binary package (`crates/pico429`) into the root Cargo workspace and extends `crates/arinc429-core` with new streaming frame opcodes (`STREAM_TX_DATA` and `STREAM_CREDIT`) along with zero-allocation constructors and unit tests. In the firmware, channel TX queues are enlarged from 16 to 128 words, the RX PubSub buffer is expanded from 1,024 to 4,096 frames, and early credit emission is added to the USB CDC tasks to preempt telemetry. On the host side, modules are constructed for OS serial port handling, a dual-lane priority scheduler with baud-rate-aware credit ceilings, an in-memory telemetry ring buffer, a CLI parser, and an embedded MCP stdio server. Finally, the legacy Python scripts are deprecated, operator documentation is updated, and the entire stack is validated via unit tests, Clippy linting, a 10,000-word loopback Built-In Test, and a 12-channel 533 KB/s streaming stress test.

---

## Comprehension Findings

### 🔴 Blocker Findings

1. **Magic Value Mismatch in Wire Protocol Specification**
   * **Scope**: Section 1 (Line 84), Section 3 (Line 128).
   * **Discrepancy**: Document claims `Magic (0xA429)`. In [`crates/arinc429-core/src/protocol.rs`](file:///c:/Users/lihan/Work/pico-429/crates/arinc429-core/src/protocol.rs#L11), the constant is `BINARY_MAGIC: [u8; 2] = [0x41, 0x29];` (ASCII `"A)"`, `0x4129`). Packet decoders following the doc will fail with `InvalidMagic`.

2. **`CMD_ACK` Field Layout and Error Code Mapping Contradiction**
   * **Scope**: Section 3.1 (Lines 142–144) vs [`crates/arinc429-core/src/protocol.rs`](file:///c:/Users/lihan/Work/pico-429/crates/arinc429-core/src/protocol.rs#L49-L56).
   * **Discrepancy**: Document claims status code is in `payload_0`, whereas wire protocol stores it in the `flags` byte (Byte 3). The numeric mappings also contradict code (`0x01` is `ERR_UNKNOWN_CMD`, `0x02` is `ERR_INVALID_PARAM`, `0x03` is `ERR_MODE_CONFLICT`, `0x05` is `ERR_BUFFER_FULL`).

3. **Undefined Data Types Across Crate Boundaries (`InboundArincFrame` & `ChannelConfig`)**
   * **Scope**: Section 5.2 (Line 296), Section 10 (Line 463).
   * **Discrepancy**: `InboundArincFrame` is internal to `crates/firmware/src/channel.rs` and not exported in `arinc429-core`. `ChannelConfig` struct referenced in MCP is never defined in the document.

---

### 🟡 Major Findings

4. **Inexact Verification Plan Target Package Name**
   * **Scope**: Verification Plan (Line 491).
   * **Discrepancy**: Document specifies `cargo build --package firmware`, but in [`crates/firmware/Cargo.toml`](file:///c:/Users/lihan/Work/pico-429/crates/firmware/Cargo.toml#L2), the package name is `pico-429-firmware`. Cargo fails immediately with `package ID specification 'firmware' did not match`.

5. **Ambiguous Tail-Flush Condition for Flow Control Credits (`STREAM_CREDIT`)**
   * **Scope**: Section 4.2.2 (Lines 229–236), Section 4 (Line 409).
   * **Discrepancy**: Triggering credit solely on 32-word consumption leaves tail batches (< 32 words) uncredited. Need an empty-queue trigger / tail-flush check.

6. **Unspecified Serial Port Discovery and Connection Ergonomics**
   * **Scope**: Section 4 (Lines 161–186), Section 5.3 (Lines 304–305).
   * **Discrepancy**: Document details auto-reconnect but never specifies port selection (`--port` argument or USB VID/PID `0x2E8A` auto-discovery).

7. **Unstated Wire Frame Endianness and Bitfield Packing Order**
   * **Scope**: Section 3 (Lines 125–136).
   * **Discrepancy**: Explicitly document big-endian wire encoding for 16-bit / 32-bit fields, and upper-nibble `[7:4]` for frame type vs lower-nibble `[3:0]` for channel.

8. **Contradiction Regarding Software vs Hardware Physical Channel Direction Control**
   * **Scope**: Section 2.1 (Line 111) vs Section 5.2 (Line 296).
   * **Discrepancy**: Clarify whether `pico429_configure_channels` synchronizes firmware state machine with board jumpers, or if mismatched directions produce `ERR_MODE_CONFLICT`.

---

### 🟢 Nit Findings

9. **Undefined Return Schemas for MCP Tools**
   * **Scope**: Section 5.2 (Lines 293–303).
   * **Discrepancy**: Define explicit response JSON structs for status, telemetry, and BIT endpoints.

10. **Unexpanded System and Domain Acronyms**
    * **Scope**: Section 2.2 (Line 118), Section 4.2 (Line 218), Section 8 (Line 447).
    * **Discrepancy**: Define `DPC` (Deferred Procedure Call), `IOCP` (I/O Completion Ports), `PPS` (Pulse Per Second), and `BIT` (Built-In Test).
