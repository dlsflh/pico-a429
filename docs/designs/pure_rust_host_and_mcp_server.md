# Pico-429 Pure Rust High-Throughput Host & Native MCP Server

*   **Author**: lihan
*   **Status**: Approved
*   **Date**: 2026-11-11

--------------------------------------------------------------------------------

## Overview

Comprehensive architectural design for the pure Rust host software stack of the Pico-429 multi-channel ARINC 429 transceiver platform. The host transitions entirely from Python prototype scripts to a unified, high-performance Rust binary (`pico429`) within the existing Cargo workspace. It natively leverages the shared `arinc429-core` protocol crate, incorporates an asynchronous streaming engine capable of handling 533 KB/s full-load bidirectional traffic with < 0.5% CPU load, and embeds an official Model Context Protocol (MCP) server (`rmcp`) to expose declarative, type-safe tools for AI coding agents.

## The Problem

The initial host software in `tools/pico429/` was designed for low-frequency command-line debugging in Python. It exhibits four structural limitations:

1. **Language Fracture & Dual-Protocol Maintenance**:
   The RP2350 firmware is written in Rust, while the initial host tools were implemented in Python. Protocol constants, 16-byte `BinaryFrame` structures, CRC algorithms, and label encoding had to be maintained redundantly in both `crates/arinc429-core` and `tools/arinc429_codec.py`. As revealed during architectural audits, this dual-maintenance led to silent opcode drift and framing discrepancies.
2. **CPython Runtime Latency & Concurrency Bottlenecks**:
   Sustaining 100 kbps full load on 6 TX and 6 RX channels requires processing 33,334 frames/second (533.3 KB/s). In Python, bitwise CRC calculation consumed 80–100% of a CPU core, creating and destroying 33,334 dataclass instances per second triggered heavy GC pressure, and the Global Interpreter Lock (GIL) introduced scheduling jitter into the USB serial data pump.
3. **Stop-and-Wait Transmission Cap**:
   The legacy Python CLI transmits individual words via synchronous `CMD_REQ` / `CMD_ACK` round-trips. Windows USB CDC (`usbser.sys`) scheduling latency (~0.5–1.0 ms RTT) caps single-word TX throughput to 1,000–2,000 words/s across all channels combined, starving 100 kbps ARINC 429 transmitters (which require 2,778 words/s per channel).
4. **Tool Calling Overhead for AI Agents**:
   AI agents lacked a typed, resident interface, requiring ad-hoc subshell invocations of Python scripts with unvalidated text parsing, high process startup overhead, and transient serial port disconnections.

## Goals

*   **100% Pure Rust Monorepo Alignment**:
    Build a unified Rust host binary crate (`crates/pico429`) in the existing Cargo workspace, sharing `crates/arinc429-core` directly with the RP2350 firmware to guarantee single-source-of-truth protocol consistency.
*   **Deterministic 533 KB/s Full-Load Pipelining**:
    Sustain continuous 33,334 words/s bidirectional ARINC 429 traffic across all 12 channels (6 TX + 6 RX at 100 kbps) with < 0.5% host CPU load, zero heap allocations on the hot path, and sub-millisecond host-to-bus queue latency.
*   **Native MCP Server via Official `rmcp` SDK**:
    Embed a first-class Model Context Protocol server directly into the binary (`pico429 mcp serve`) using the official `rmcp` crate, providing compile-time JSON Schema generation (`schemars`) and type-safe tool execution over `stdio`.
*   **Cumulative Sliding-Window Flow Control**:
    Replace per-word synchronous ACK with a self-healing cumulative consumption window protocol, preventing buffer overruns and eliminating pipeline stalls.
*   **Dual-Lane Priority Scheduling**:
    Maintain an expedited command lane (`CMD_REQ`) bypassing stream flow control to guarantee < 1.0 ms latency for interactive commands during full-rate streaming.
*   **Single Self-Contained Binary Distribution**:
    Produce a standalone, zero-dependency executable (`pico429.exe`, ~2–4 MB) eliminating Python runtime and virtual environment prerequisites.

## Non-Goals

*   **Embedded GUI Development**:
    No graphical desktop frontend (egui/iced) will be implemented in this phase. Visualization and monitoring are exposed via CLI terminal dashboards and MCP agent inspection.
*   **Alternative Physical Transports**:
    Transports remain USB Full Speed CDC-ACM. Wi-Fi and Ethernet remain out of scope.
*   **Custom Firmware Waveform Synthesis**:
    Sub-microsecond ARINC 429 bit timing and bipolar RZ waveform generation remain exclusively the responsibility of RP2350 PIO state machines.

## Open Questions

*   **Question 1 (Python Scripting Interoperability)**:
    Should the host provide optional PyO3 / `maturin` bindings in a sub-crate for avionics engineers who write Python test scripts, or is the standard CLI / JSON-RPC / IPC interface sufficient for all external scripting?
    *Resolution*: Standard CLI and native MCP over stdio fulfill primary automated agent and pipeline workflows. PyO3 bindings remain an optional future improvement.
*   **Question 2 (Long-Running MCP Job Semantics)**:
    Should streaming injection of >10,000 words in MCP return an asynchronous `{job_id}` handle with progress polling, or stream synchronously with timeout?
    *Resolution*: Default to synchronous execution with a configurable `timeout_ms` (default 5000 ms), completing short tests (< 10k words) in a single turn. For open-ended or long-running streams, `StreamJobManager` manages asynchronous execution returning a `{job_id}` polled via `pico429_stream_status`.

## Critical User Journeys

### CUJ 1: 12-Channel Full-Rate Streaming Stress Test
*   **Given**: Pico-429 hardware (Raspberry Pi Pico 2 W, RP2350A) connected over USB. 6 channels set to TX (100 kbps) and looped back to 6 RX channels (100 kbps). Host compiled in release mode.
*   **When**: User runs `pico429 stream --channels 0..5 --rate 2778 --count 100000`.
*   **Then**: 100,000 words are streamed and looped back (33,334 words/s aggregate load, 533.3 KB/s payload); 0 words dropped, 0 sequence errors; CPU utilization remains < 0.5%; per-batch round-trip latency < 2.0 ms.

### CUJ 2: Autonomous AI Agent Hardware Loopback Diagnostic via MCP
*   **Given**: AI coding agent (Claude Code / Cursor / Antigravity) running with `pico429 mcp serve` configured in MCP settings over `stdio`.
*   **When**: Agent executes `pico429_run_bit(tx_channel=0, rx_channel=1, count=1000)`.
*   **Then**: Rust MCP server initiates the hardware loopback, validates received words and parity in memory, and returns structured JSON verification results directly to the agent without subshell spawning or terminal text scraping.

--------------------------------------------------------------------------------

## Technical Plan

### 1. Pure Rust Monorepo Architecture

The entire codebase is structured as a unified Cargo workspace under `pico-429/`, eliminating language boundaries, serialization discrepancies, and duplicated protocol models:

```text
+-----------------------------------------------------------------------------+
|                           Cargo Monorepo Workspace                          |
|                                                                             |
|   +---------------------------------------------------------------------+   |
|   |                         crates/arinc429-core                        |   |
|   |   (no_std, zero-heap-allocation protocol, codecs, parity, labels)    |   |
|   |   - BinaryFrame (16B Copy struct, magic 0x4129, CRC-16-CCITT)       |   |
|   |   - Protocol Opcodes (0x01..0x06), Frame Types, Stream Encoders     |   |
|   |   - Label (Octal/Binary), Filter Bitmaps, Profile Serialization     |   |
|   +----------------------------------+----------------------------------+   |
|                                      |                                      |
|                 +--------------------+--------------------+                 |
|                 | (Compile-time dependency)               |                 |
|                 v                                         v                 |
|   +-----------------------------+       +-------------------------------+   |
|   |       crates/firmware       |       |        crates/pico429         |   |
|   | (Pico 2 W Embassy Firmware) |       |   (Unified Pure-Rust Host)    |   |
|   | - 24 GPIO Channel Pinout    |       | - CLI (clap derive)           |   |
|   | - 12x PIO State Machines    |       | - Streaming Engine (Tokio)    |   |
|   | - 128-word Queue per Channel|       | - Official MCP Server (rmcp)  |   |
|   | - Embassy USB CDC Service   |       | - Windows Serial (serialport) |   |
|   +-----------------------------+       +-------------------------------+   |
+-----------------------------------------------------------------------------+
```

### 2. Hardware Topology & Platform Roadmap

#### 2.1 Active Hardware Baseline: Raspberry Pi Pico 2 W (RP2350A)
The active production and verification target remains the **Raspberry Pi Pico 2 W** development board powered by the RP2350A microcontroller (30 GPIO pins total, DIP-40 form factor). The existing pin assignments remain strictly preserved:

*   **GPIO 0..21, 26, 27 (24 pins)**: 12 physical ARINC 429 differential transceiver pairs (Channel 0 through Channel 11), driven directly by RP2350 PIO state machines partitioned across PIO0, PIO1, and PIO2.
*   **GPIO 25 (1 pin)**: Dedicated hardware line (`WL_CS`) connected to the on-module Infineon CYW43439 wireless chip. In the bare-metal firmware, GPIO 25 is held permanently high in output mode to keep the wireless chip idle and deselect its SPI interface.
*   **Board Status LED**: Inactive on Pico 2 W. Because the physical LED is connected to GPIO 0 of the CYW43439 Wi-Fi chip rather than an RP2350 GPIO pin, toggling it requires linking the full wireless networking driver stack (~300 KB binary footprint and continuous polling overhead). LED functionality is intentionally disabled in the current firmware build.
*   **Channel Direction Switching**: Fixed / hardwired on the current carrier board (or configured via hardware jumpers / DIP switches) due to GPIO pin constraints on the 30-pin RP2350A.

#### 2.2 Future Hardware Migration Roadmap (RP2350B TODO)
As a planned hardware iteration, migrating to custom boards based on the **RP2350B** chip (QFN-80 package with 48 full GPIOs) is scheduled in the project backlog:
*   **GPIO 0..23 (24 pins)**: 12-channel ARINC 429 differential pairs (PIO transceivers).
*   **GPIO 24..35 (12 pins)**: 12-channel independent physical TX/RX direction switching (driving analog switch / relay gates directly from native MCU pins without external I2C GPIO expander ICs).
*   **GPIO 36 (1 pin)**: Native board status LED (directly driven by RP2350B GPIO, zero wireless stack dependency).
*   **GPIO 37..47 (11 pins)**: External hardware trigger inputs, PPS time-synchronization pulses, and dedicated hardware debug UART.

### 3. Exact Wire Protocol Specification

Communication over USB CDC uses fixed 16-byte `BinaryFrame` packets ($4 \times 16 = 64$ bytes per USB Full Speed transaction). All structures are natively defined in `crates/arinc429-core/src/protocol.rs`:

#### 3.1 Frame Layout

```text
 0                   1                   2                   3
 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|       Magic (0x4129)          | Type  | Chan  |     Flags     |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|        Sequence Number        |    Payload 0 (Bits 0..15)     |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|    Payload 0 (Bits 16..31)    |    Payload 1 (Bits 0..15)     |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|    Payload 1 (Bits 16..31)    |         CRC-16-CCITT          |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
```

#### 3.2 Frame Types Definition
*   `0x01` (`ARINC_DATA`): Downlink telemetry frame.
    *   `payload_0`: 32-bit hardware reception timestamp ($\mu\text{s}$).
    *   `payload_1`: 32-bit raw ARINC 429 word.
*   `0x02` (`CMD_ACK`): Synchronous command acknowledgment.
    *   `flags`: Command execution status code matching `arinc429_core::protocol::ack_status` (`0x00` = OK, `0x01` = ErrUnknownCmd, `0x02` = ErrInvalidParam, `0x03` = ErrModeConflict, `0x04` = ErrFlash, `0x05` = ErrBufferFull).
    *   `payload_0`: Command-specific return value / query response data 0.
    *   `payload_1`: Command-specific return value / query response data 1.
*   `0x03` (`ASYNC_EVENT`): Asynchronous hardware event / error alert.
*   `0x04` (`CMD_REQ`): Synchronous control request (configuration, mode change, BIT execution).
    *   `flags`: Command opcode (`PING = 0x01`, `SET_CHANNEL_CFG = 0x02`, `SET_FILTER = 0x03`, `TX_RAW_WORD = 0x04`, `SET_OPERATING_MODE = 0x05`, `CFGBATCH = 0x07`, `QUERY_STATUS = 0x08`, `PROFILE_OP = 0x09`, `REBOOT_TO_BOOTSEL = 0x0A`, `ABORT_TX = 0x0B`).
    *   `ABORT_TX` (`0x0B`): Purges channel transmit FIFO (`CHANNEL_TX_QUEUES[channel]`), resets in-flight credit tracking, and immediately halts pending PIO output on that channel.
*   `0x05` (`STREAM_TX_DATA`): Asynchronous outbound data word.
    *   `channel`: Target channel ID (0..11, 4-bit nibble).
    *   `flags`: Bit 0 = odd parity validation enable.
    *   `seq_num`: Monotonic sequence tag for gap auditing.
    *   `payload_0`: 32-bit raw ARINC 429 word to transmit.
    *   `payload_1`: Host timestamp or sub-channel routing tag.
    *   *Behavior*: Enqueued directly into channel TX queue in firmware without per-word `CMD_ACK`.
*   `0x06` (`STREAM_CREDIT`): Asynchronous flow control credit update.
    *   `channel`: Channel ID (0..11, 4-bit nibble).
    *   `flags`: Reserved (`0x00`).
    *   `seq_num`: Monotonic credit frame counter.
    *   `payload_0`: Available FIFO slots in firmware `CHANNEL_TX_QUEUES[channel]` (0..128).
    *   `payload_1`: Cumulative 32-bit counter of handled ARINC words ($\text{ConsumedByHardware} + \text{DroppedWords}$).

### 4. High-Throughput Rust Streaming Engine (`crates/pico429/src/transport/`)

The streaming engine is built on Tokio and native OS threads:

```text
                               Host Application / MCP
                                         |
                       +-----------------+-----------------+
                       | (Expedited Lane)                  | (Streaming Lane)
                       v                                   v
             [Priority Command Queue]             [Stream Batch Queue]
             (CMD_REQ: PING, CFG)                 (STREAM_TX_DATA, 64B)
                       |                                   |
                       | (Preempts bulk data)              | (Gated by Credits)
                       +-----------------+-----------------+
                                         |
                                         v
                         +-------------------------------+
                         |   Transport Worker (Tokio)    |
                         |   - 64-byte batch packing     |
                         |   - Adaptive 250us timer      |
                         |   - Sliding-window tracker    |
                         +---------------+---------------+
                                         |
                                         v OS Serial (serialport-rs)
                            USB 1.1 Bulk (64B packets)
                                         |
                                         v
                         +-------------------------------+
                         |   Pico-429 RP2350 Firmware    |
                         |   - AlignedStreamBuf (1024B)  |
                         |   - CHANNEL_TX_QUEUES (128w/ch|
                         |   - Early Credit: drain 32w   |
                         |     (96w / 34.5ms runway res) |
                         +-------------------------------+
```

#### 4.1 Dual-Lane Priority Scheduler (Control Plane vs Data Plane Decoupling)

Under full load (6 TX channels at 100 kbps), the host streams 16,667 words/second into the USB transport. In a naive single-FIFO design, mixing control frames and streaming data frames creates two fatal failure modes:
1.  **Head-of-Line Blocking**:
    If an interactive user or AI agent issues a control request (such as `pico429_get_status` health check, channel baud rate reconfiguration, or emergency cancellation `pico429_abort_stream`) while a large streaming test (>10,000 words) is active, the control frame would be queued behind thousands of streaming words. At 16.6k words/s, a queue backlog of just 500 words delays the control command by 30 ms (violating the < 1.0 ms interactive latency requirement).
2.  **Credit-Exhaustion Deadlock**:
    Streaming bulk data is gated by sliding-window flow control (`STREAM_CREDIT`). If the firmware's 128-word hardware queue fills up, available credits drop to zero (`UsableCredit == 0`), and the streaming queue pauses. If an emergency stop or cancellation command (`pico429_abort_stream`) is placed in that same paused queue, it can never be dispatched, deadlocking the entire system because the host cannot issue commands to unblock or abort the stream.

To eliminate both risks, the engine implements a dual-lane priority scheduler with strict decoupling of control and data planes:
*   **Lane 0 (Expedited Control Plane Lane)**:
    *   **Traffic Class**: Exclusively synchronous control requests (`CMD_REQ`, opcode `0x04`: ping, configure, status, and `ABORT_TX`). Single data words (`pico429_send_word`) are routed to the Data Plane (Lane 1) or dispatched when no active stream conflicts.
    *   **Flow Control**: Bypasses streaming credit gating completely. Control frames are processed directly by the MCU core in `usb_service.rs` without entering channel FIFOs, ensuring they are never blocked by data buffer backpressure.
    *   **Scheduling Preemption**: Preempts the streaming batch queue. When a frame arrives in Lane 0, the transport worker dispatches it immediately as a short USB packet (16 or 32 bytes) and invokes an immediate USB endpoint flush, guaranteeing < 1.0 ms round-trip execution.
*   **Lane 1 (Credit-Gated Streaming Data Lane)**:
    *   **Traffic Class**: High-throughput continuous data words (`STREAM_TX_DATA`, opcode `0x05`) and interactive data words (`pico429_send_word`).
    *   **Flow Control**: Strictly gated by per-channel cumulative sliding-window credits (`UsableCredit[ch] > 0`).
    *   **Aggregation**: Accumulated into 64-byte chunks ($4 \times 16\text{ bytes} = 64\text{ bytes}$) to maximize USB Full Speed bus bandwidth. If fewer than 4 frames are queued, an adaptive micro-timer ($\le 250\ \mu\text{s}$) flushes partial packets to prevent sparse or low-baud (12.5 kbps) streams from stalling.

#### 4.2 Anti-Jitter Sliding-Window Flow Control & Memory Allocation Architecture

To guarantee zero buffer overruns, zero credit leakage, and bounded jitter tolerance up to 34.56 ms against Windows USB DPC and thread scheduling jitter, the host and firmware implement a multi-tiered flow control and memory architecture:

##### 4.2.1 Cumulative Accounting, Baseline Latching & Self-Healing Window
To prevent unsigned 32-bit underflow on host reconnection, the host latches baseline consumption upon initial connection:
*   **Connection Bootstrap**: Firmware broadcasts an initial `STREAM_CREDIT` frame for all active TX channels upon USB CDC host enumeration, reporting 128 available FIFO slots and current cumulative handled words.
*   **Baseline Latching**: Upon receiving the bootstrap credit frame, the host latches $\text{BaselineConsumed}[ch] = \text{ReportedConsumed}[ch]$.
*   **Relative In-Flight Accounting**:
    $$\text{RelativeConsumed}[ch] = \text{ReportedConsumed}[ch].\text{wrapping\_sub}(\text{BaselineConsumed}[ch])$$
    $$\text{InFlight}[ch] = \text{SentCount}[ch] - \text{RelativeConsumed}[ch]$$
    $$\text{UsableCredit}[ch] = \min(\text{MaxCreditCeiling}[ch] - \text{InFlight}[ch], \text{ReportedAvailableSlots}[ch])$$
*   **Hardware Drop Accounting & Zero Credit Leakage**: Firmware tracks `DROPPED_TX_WORDS: [AtomicU32; ACTIVE_CHANNEL_COUNT]`. When a word cannot be enqueued into `CHANNEL_TX_QUEUES[ch]` (e.g. queue full or channel disabled), firmware increments `DROPPED_TX_WORDS[ch]`. Reported cumulative handled words is:
    $$\text{ReportedConsumed}[ch] = \text{ConsumedByHardware}[ch].\text{wrapping\_add}(\text{DroppedWords}[ch])$$
    This ensures every frame sent by the host is accounted for, preventing creeping credit exhaustion.
*   **Self-Healing**: Dropped or delayed credit frames do not corrupt state because subsequent `STREAM_CREDIT` frames carry the authoritative cumulative counter from hardware.

##### 4.2.2 Early Refill Watermark & Anti-Starvation Runway
At 100 kbps, 1 ARINC word takes $360\ \mu\text{s}$. If refill is only signaled when the buffer is empty or half-drained, Windows USB scheduling jitter (p99.9 spikes of 10~15 ms due to DPCs, background services, or thread preemption) will drain the queue to zero, causing transmission underflow (starvation bubbles on the physical wire).

*   **Dynamic Watermark Scaling**: Early credit threshold is dynamically scaled to $\min(32, \text{MaxCreditCeiling}[ch] / 2)$:
    *   **100 kbps (Ceiling 128)**: Threshold is **32 words**, leaving **96 words in reserve**.
        $$T_{\text{runway}} = 96 \times 360\ \mu\text{s} = 34.56\text{ ms}$$
        Providing $>2.3\times$ safety margin over the worst-case $15\text{ ms}$ Windows scheduling spike.
    *   **12.5 kbps (Ceiling 32)**: Threshold is **8 words**, leaving **24 words in reserve** ($24 \times 2.88\text{ ms} = 69.12\text{ ms}$ runway).
*   **Queue-Empty Tail Flush**: If `CHANNEL_TX_QUEUES[ch].is_empty()` and uncredited handled words $> 0$, the firmware emits `STREAM_CREDIT` immediately without waiting to cross the watermark, guaranteeing sparse bursts (< 32 words) never stall.

##### 4.2.3 Baud-Rate Aware Dynamic Credit Ceilings
To prevent the low-speed 12.5 kbps mode ($2.88\text{ ms}$ per word) from accumulating seconds of bufferbloat, the host dynamically caps credits based on active channel baud rate:

| Baud Rate Mode | Physical Word Time | Host Credit Ceiling | Max Hardware Queue Latency | Emergency Abort Brake Time |
| :--- | :--- | :--- | :--- | :--- |
| **100 kbps (High Speed)** | $360\ \mu\text{s}$ | **128 words** | $46.08\text{ ms}$ | $< 0.05\text{ s}$ physical bus silence |
| **12.5 kbps (Low Speed)** | $2.88\text{ ms}$ | **32 words** | $92.16\text{ ms}$ | $< 0.10\text{ s}$ physical bus silence |

##### 4.2.4 Firmware Memory Allocation Matrix
With RP2350 providing 520 KB SRAM and current firmware occupying < 25 KB, buffer memory is scaled generously for passive capture while kept bounded for active transmission:

| Buffer Component | Location | Allocated Capacity | SRAM Footprint | Engineering Rationale |
| :--- | :--- | :--- | :--- | :--- |
| `CHANNEL_TX_QUEUES` | `channel.rs` | **128 words / channel** | 6.0 KB (12 channels) | Provides 34.56 ms runway @ 100k; costs only 1.1% of SRAM. |
| `GLOBAL_RX_PUBSUB` | `channel.rs` | **4096 frames** | 64.0 KB | Passive telemetry capture cannot be backpressured. Holds **245.7 ms** of full-load 6-channel RX data (16,667 f/s), eliminating dropped frames during host OS hiccups. CDC write timeout is set to $\ge 300\text{ ms}$ to match this absorption capacity. |
| `AlignedStreamBuf` | `usb_service.rs` | **1024 bytes** | 1.0 KB | Holds 16x 64B USB packets; eliminates `copy_within` thrashing. |
| `USB_CMD_RESP_QUEUE` | `usb_service.rs` | **32 frames** | 0.5 KB | Dedicated exclusively to synchronous `CMD_ACK` dispatch, eliminating frame drop risks. |
| `CREDIT_DIRTY_MASK` | `channel.rs` | **AtomicU16** | 2 bytes | Lock-free bitmask coalescing channel credit triggers directly into 64B CDC batch packets without queue overhead. |
| **Total SRAM Allocated** | — | — | **~71.5 KB** | Consumes only **13.7%** of RP2350's 520 KB SRAM; leaves > 440 KB free. |

### 5. Native Model Context Protocol (MCP) Server

The host binary embeds an official Model Context Protocol server powered by the official `rmcp` SDK (v3.0):

#### 5.1 Declarative Tool Registration
Tools are declared using standard Rust async methods with `#[tool]` attributes and type-checked parameter structs deriving `schemars::JsonSchema`:

```rust
#[derive(Deserialize, JsonSchema)]
struct SendWordParams {
    /// Target channel index (0..11)
    channel: u8,
    /// 8-bit octal ARINC 429 label (0..255)
    label: u8,
    /// 32-bit ARINC 429 data word payload
    data: u32,
    /// SDI bits (0..3)
    sdi: Option<u8>,
    /// SSM bits (0..3)
    ssm: Option<u8>,
    /// Parity mode ("odd" | "even" | "none")
    parity: Option<String>,
}

#[tool_router(server_handler)]
impl Pico429McpServer {
    #[tool(description = "Transmit a single validated ARINC 429 word with flow-control verification")]
    async fn pico429_send_word(&self, Parameters(p): Parameters<SendWordParams>) -> Result<CallToolResult, McpError> {
        let word = arinc429_core::word::encode_word(p.label, p.data, p.sdi.unwrap_or(0), p.ssm.unwrap_or(0), p.parity.as_deref().unwrap_or("odd"))?;
        self.engine.send_word(p.channel, word).await?;
        Ok(CallToolResult::success(vec![Content::text("Word transmitted successfully")]))
    }
}
```

#### 5.2 MCP Tool Endpoints Specification

| Tool Name | Parameters | Description |
| :--- | :--- | :--- |
| `pico429_get_status` | None | Returns device hardware status, channel directions, link baud rates, buffer levels, and gap counters. |
| `pico429_configure_channels` | `channels`: list[ChannelConfig] | Configures operating modes, speeds (100k/12.5k), and directions for all 12 channels. |
| `pico429_send_word` | `channel`: u8, `label`: u8, `data`: u32, `sdi`?: u8, `ssm`?: u8, `parity`?: str | Transmits a single validated ARINC 429 word via the data plane with credit verification. |
| `pico429_stream_send` | `channel`: u8, `words`: list[u32], `rate_hz`: u32, `timeout_ms`?: u64, `async_mode`?: bool | Injects a stream of ARINC 429 words using credit-paced batching. By default, executes synchronously within `timeout_ms` (default 5000 ms) returning `{status: "completed", words_sent, elapsed_ms}`; if `async_mode: true` or duration exceeds timeout, returns `{job_id, status: "started"}` for background polling. |
| `pico429_stream_status` | `job_id`: str | Returns streaming progress, words sent, queue depth, and backpressure metrics. |
| `pico429_abort_stream` | `channel`: u8, `job_id`?: str | Dispatches expedited `CMD_REQ: ABORT_TX (0x0B)` to purge firmware FIFO, cancels active host streaming task, and resets in-flight credits. |
| `pico429_fetch_telemetry` | `channel`: u8, `limit`: usize = 100, `label_filter`: Option\<u8\> | Fetches recent decoded ARINC 429 telemetry from the in-memory bounded ring buffer. |
| `pico429_get_latest_labels` | `channel`: u8, `labels`: Option\<list[u8]\> | Fetches latest received values for specified labels (or all active labels) directly from host Current Value Table (CVT) in $O(1)$ time, eliminating ring-buffer eviction issues during slow agent turns. |
| `pico429_run_bit` | `tx_channel`: u8, `rx_channel`: u8, `count`: u32 = 1000 | Runs an autonomous hardware loopback Built-In Test in host memory, validating 100% of received words in real time. |

#### 5.3 Stdio Transport Integration
AI agents (Claude Code, Cursor, Antigravity) spawn `pico429 mcp serve` directly over `stdio`. The process maintains the USB serial connection for the duration of the agent session.

--------------------------------------------------------------------------------

## Alternatives Considered

### Alternative A: High-Performance Python Daemon with FastMCP (`asyncio` + `pyserial-asyncio`)

*   **Pros**: Rapid scripting turnaround in Python; allows developers to leverage existing Python test assets and interactive Jupyter/REPL workflows.
*   **Cons**:
    *   **Language Fracture**: RP2350 firmware is in Rust, requiring dual-maintenance of 16-byte `BinaryFrame` definitions, CRC-16 algorithms, and opcode enumerations across `crates/arinc429-core` and `tools/arinc429_codec.py`.
    *   **CPython Runtime Overhead**: Processing 33,334 frames/s (533.3 KB/s aggregate load) consumes 80–100% of a CPU core in Python due to per-frame CRC calculations and object allocations, while garbage collection (GC) pauses introduce jitter into the USB serial data pump.
    *   **Packaging Friction**: Requires end users and automated CI runners to configure Python runtimes, virtual environments, and platform-specific dependencies.
*   **Reason for Rejection**: `Rejected by author: Redundant protocol definition in two languages caused silent opcode drift during development. CPython runtime overhead and GIL jitter prevent reliable 533 KB/s full-load streaming, and virtual environment setup adds friction for host deployment.`

### Alternative B: Hybrid Architecture (PyO3 Rust Extension Core with Python FastMCP)

*   **Pros**: Offloads CPU-intensive CRC computation and framing to compiled Rust; retains Python FastMCP tool ergonomics.
*   **Cons**:
    *   **FFI Marshalling Overhead**: Crossing the PyO3 FFI boundary 33,334 times per second to convert Rust binary structs into Python dataclasses reintroduces memory allocations and GC overhead on the hot path.
    *   **Build Complexity**: Requires a dual build toolchain combining Cargo and `maturin`, requiring MSVC build tools for C-extension compilation on Windows.
    *   **Runtime Dependencies**: Still requires a Python interpreter, wheels, and `.venv` setup on host machines.
*   **Reason for Rejection**: `Rejected by author: PyO3 FFI conversions on the 33,334 frames/s hot path negate zero-copy performance advantages. Furthermore, the official Rust MCP SDK (rmcp) provides equivalent macro-based ergonomics (#[tool]) without any Python runtime dependencies.`

### Alternative C: Firmware-Autonomous Streaming via External PSRAM Buffering

*   **Pros**: Eliminates continuous high-speed USB data streaming by storing pre-generated transmission patterns directly in microcontroller memory.
*   **Cons**:
    *   **Hardware Incompatibility**: Active target hardware is Raspberry Pi Pico 2 W, which does not have an external PSRAM chip populated (only 520 KB on-chip SRAM).
    *   **Static Waveform Rigidity**: Pre-baked transmission buffers cannot support dynamic, closed-loop Hardware-in-the-Loop (HIL) simulations where transmitted words must react in real time to received telemetry.
    *   **Loss of Agent Interactivity**: AI agents cannot conditionally inject ARINC 429 frames or adjust flight simulation parameters dynamically.
*   **Reason for Rejection**: `Rejected by author: Incompatible with active Pico 2 W hardware (no PSRAM chip). More critically, static pattern playback prevents interactive, reactive closed-loop testing required by AI agents and real-time flight software validation.`

### Alternative D: Custom WinUSB / libusb Bulk Driver instead of USB CDC-ACM Virtual Serial

*   **Pros**: Bypasses the OS serial driver stack (`usbser.sys`), offering minor kernel-level latency reductions.
*   **Cons**:
    *   **Driver Installation Friction**: Requires users to install third-party driver packages or execute Zadig to overwrite default Windows composite USB drivers, creating severe deployment friction.
    *   **Sufficient CDC-ACM Throughput**: With 64-byte batch packing, USB CDC-ACM achieves > 800 KB/s throughput and sub-millisecond round-trip latency on Windows 10/11, macOS, and Linux without requiring any driver installation.
*   **Reason for Rejection**: `Rejected by author: Driver installation friction on Windows (requiring Zadig or signed INF) is unacceptable for general deployment. Standard USB CDC-ACM with 64-byte aligned transfers satisfies all throughput and latency targets out-of-the-box.`

--------------------------------------------------------------------------------

## Non-Functional Requirements

### 1. Performance & Computational Efficiency
*   **Throughput**: Sustained continuous bidirectional 533.3 KB/s USB payload transfer (33,334 words/s) with host process CPU utilization < 0.5% on modern multicore x86/ARM machines.
*   **Memory Allocations**: Zero heap allocations on the hot streaming path (`BinaryFrame` is `Copy`).
*   **Latency**: Host-to-bus streaming queue latency < 2.0 ms; expedited command response < 1.0 ms.
*   **Anti-Starvation Runway**: Firmware maintains $\ge 34.56\text{ ms}$ buffer runway under 100 kbps full load via early credit signalling (at 32 words drained), absorbing Windows USB DPC and thread scheduling spikes up to 20 ms with zero inter-message transmission underflow.

### 2. Reliability & Resilience
*   **USB Disconnect Recovery**: Upon USB disconnect, the host transitions to disconnected state, holds pending requests, detects re-enumeration, re-opens COM handle, replays cached channel configurations and filters, and resynchronizes credits before resuming.
*   **Buffer Bounding & Fault Isolation**: Firmware buffer allocation is strictly bounded to 71.5 KB SRAM (13.7% of RP2350's 520 KB), with 64.0 KB dedicated to `GLOBAL_RX_PUBSUB` (4096 frames) to absorb up to 245.7 ms of full-rate passive telemetry without loss during host OS interrupts. Firmware CDC write timeout is set to $\ge 300\text{ ms}$ to honor this absorption capacity.
*   **Host Ring Buffer Auditing**: Host telemetry buffers are fixed-depth (`VecDeque` depth 32,768 per channel). Monotonic `dropped_words_counter` and `sequence_gap_counter` explicitly flag overwrites.
*   **Telemetry Freshness (Current Value Table)**: Host maintains an in-memory `CurrentValueTable` (`[AtomicU32; 256]` per channel) capturing the latest value and timestamp of every label. This decouples AI agent turn latency (10–20s) from ring buffer depth, guaranteeing $O(1)$ state retrieval without buffer overwrite risk.

--------------------------------------------------------------------------------

## Additional Resources

*   `crates/arinc429-core/src/protocol.rs`: Shared 16-byte `BinaryFrame` and opcode definitions.
*   `crates/firmware/src/usb_service.rs`: RP2350 USB CDC-ACM packet handling and state machine.
*   `crates/firmware/src/channel.rs`: 12-channel routing, hardware queues, and telemetry pubsub.
*   Anthropic Model Context Protocol (MCP) Rust SDK: `rmcp` (v3.0).

## Future Improvements

*   **RP2350B Hardware Migration**: Transition from Raspberry Pi Pico 2 W (RP2350A, 30 GPIOs) to custom board utilizing the RP2350B (QFN-80 package with 48 full GPIOs) to natively drive 12-channel direction relays, native status LED, and hardware triggers.
*   **Optional PyO3 C-Extension Wrapper**: If avionics test teams require native Python scripting bindings, export an optional PyO3 sub-crate (`crates/pico429-python`) linking against the Rust streaming engine.
*   **DMA Double-Buffering for USB CDC**: Explore Embassy USB zero-copy DMA endpoint transfers once Embassy RP2350 USB DMA matures, reducing USB CDC CPU interrupts in firmware.

--------------------------------------------------------------------------------

## Detailed Implementation Plan

### Target Files to Create/Modify

#### 1. `Cargo.toml` (Modify)
*   **Type / Scope**: Workspace Root Manifest
*   **Proposed Changes**:
    *   Add `"crates/pico429"` to `workspace.members` array to register the host binary crate.
    *   Add package-specific profile override `[profile.release.package.pico429] opt-level = 3` to maximize host throughput without altering firmware binary footprint.

#### 2. `crates/arinc429-core/src/protocol.rs` (Modify)
*   **Type / Scope**: Shared Core Protocol Library
*   **Proposed Changes**:
    *   Define opcode constants: `frame_types::STREAM_TX_DATA = 0x05`, `frame_types::STREAM_CREDIT = 0x06`, and `opcodes::ABORT_TX = 0x0B`.
    *   Implement helper constructor `BinaryFrame::new_stream_tx(channel: u8, seq: u16, word: u32, timestamp_or_tag: u32, odd_parity: bool) -> Self`.
    *   Implement helper constructor `BinaryFrame::new_stream_credit(channel: u8, seq: u16, available_slots: u32, cumulative_handled: u32) -> Self`.
    *   Implement helper constructor `BinaryFrame::new_cmd_abort_tx(channel: u8, seq: u16) -> Self`.
    *   Expose zero-copy payload accessors for extracting stream payloads and credit counters.

#### 3. `crates/arinc429-core/src/word.rs` (Modify)
*   **Type / Scope**: Shared Core Word Encoding & Parity
*   **Proposed Changes**:
    *   Implement `encode_word(label: u8, data: u32, sdi: u8, ssm: u8, parity: &str) -> Result<u32, ArincWordError>`.
    *   Ensure odd/even/none parity bit generation complies with ARINC 429 Part 1 specifications.

#### 4. `crates/arinc429-core/tests/stream_protocol_tests.rs` (Create)
*   **Type / Scope**: Protocol Unit Tests
*   **Proposed Changes**:
    *   Validate 16-byte exact memory layout and alignment of `STREAM_TX_DATA` and `STREAM_CREDIT` frames.
    *   Verify CRC-16-CCITT calculations and bit-level corruption rejection.
    *   Validate 32-bit sequence number wrap-around handling and credit accounting edge cases.

#### 5. `crates/firmware/src/channel.rs` (Modify)
*   **Type / Scope**: Embedded Channel Routing & Queue Buffers
*   **Proposed Changes**:
    *   Expand `CHANNEL_TX_QUEUES` capacity from 16 to 128 words per channel: `[Channel<CriticalSectionRawMutex, u32, 128>; ACTIVE_CHANNEL_COUNT]`.
    *   Expand `GLOBAL_RX_PUBSUB` capacity from 1024 to 4096 inbound frames: `PubSubChannel<CriticalSectionRawMutex, InboundArincFrame, 4096, 1, ACTIVE_CHANNEL_COUNT>`.
    *   Add monotonic 32-bit `cumulative_consumed_counter: [AtomicU32; ACTIVE_CHANNEL_COUNT]` incremented as words are dispatched to PIO.
    *   Add monotonic 32-bit `dropped_tx_words: [AtomicU32; ACTIVE_CHANNEL_COUNT]` incremented when channel queue is full or disabled.
    *   Add `CREDIT_DIRTY_MASK: AtomicU16` tracking channels with pending credit updates for lock-free coalescing.
    *   Implement dynamic early watermark threshold $\min(32, \text{ceiling}/2)$ and queue-empty tail-flush trigger setting `CREDIT_DIRTY_MASK`.

#### 6. `crates/firmware/src/driver/device.rs` (Modify)
*   **Type / Scope**: Embedded Hardware PIO Driver & Transmit Worker
*   **Proposed Changes**:
    *   Transmit worker pops words from `CHANNEL_TX_QUEUES[ch]`, feeds PIO state machine FIFO, and increments `cumulative_consumed_counter[ch]`.
    *   On queue drain or watermark crossing, sets bit `(1 << ch)` in `CREDIT_DIRTY_MASK`.
    *   Implement channel TX queue purge and PIO abort logic on receipt of `opcodes::ABORT_TX`.

#### 7. `crates/firmware/src/usb_service.rs` (Modify)
*   **Type / Scope**: Embedded USB CDC Communication Engine
*   **Proposed Changes**:
    *   Expand `AlignedStreamBuf` from 128 bytes to 1024 bytes (supporting 16 full 64-byte USB packets).
    *   Expand `USB_CMD_RESP_QUEUE` from 16 to 32 frames (dedicated exclusively to `CMD_ACK` frames).
    *   Increase USB CDC endpoint write timeout from 100 ms to $\ge 300\text{ ms}$ to match `GLOBAL_RX_PUBSUB` absorption window.
    *   Upon USB CDC enumeration / connection bootstrap, broadcast initial `STREAM_CREDIT` frame for all active TX channels.
    *   In `usb_cdc_rx_task`: Pattern match `frame_types::STREAM_TX_DATA`; enqueue directly to `send_to_channel_tx(channel, payload_0)` without per-word `CMD_ACK`. If queue full, increment `dropped_tx_words[channel]`.
    *   In `usb_cdc_tx_task`: Read and clear `CREDIT_DIRTY_MASK` via `fetch_and(0, Ordering::AcqRel)`, packing coalesced credit frames directly into 64-byte batch transfers before transmitting bulk telemetry.

#### 8. `crates/pico429/Cargo.toml` (Create)
*   **Type / Scope**: Host Binary Package Manifest
*   **Proposed Changes**:
    *   Dependencies:
        *   `arinc429-core = { path = "../arinc429-core" }`
        *   `tokio = { version = "1.43", features = ["full"] }`
        *   `tokio-util = "0.7"`
        *   `serialport = "4.7"`
        *   `clap = { version = "4.5", features = ["derive"] }`
        *   `rmcp = { version = "3.0", features = ["server", "macros", "transport-io"] }`
        *   `schemars = "0.8"`
        *   `serde = { version = "1.0", features = ["derive"] }`
        *   `serde_json = "1.0"`
        *   `tracing = "0.1"`
        *   `tracing-subscriber = "0.3"`
        *   `indicatif = "0.17"`
        *   `uuid = { version = "1.11", features = ["v4"] }`
        *   `anyhow = "1.0"`
    *   Platform-specific dependencies:
        *   `[target.'cfg(windows)'.dependencies] windows = { version = "0.58", features = ["Win32_Media"] }`

#### 9. `crates/pico429/src/lib.rs` (Create)
*   **Type / Scope**: Host Library Root
*   **Proposed Changes**:
    *   Declare and expose public modules: `pub mod transport; pub mod telemetry; pub mod mcp;`.
    *   Enables integration tests in `crates/pico429/tests/` to link directly against host scheduling and protocol engines.

#### 10. `crates/pico429/src/main.rs` (Create)
*   **Type / Scope**: Host CLI Entry Point & Command Dispatcher
*   **Proposed Changes**:
    *   CLI definition via `clap(Parser)` with subcommands: `status`, `config`, `send`, `stream`, `telemetry`, `bit`, `mcp serve`.
    *   On Windows, call `timeBeginPeriod(1)` on startup and `timeEndPeriod(1)` on shutdown to guarantee sub-millisecond OS timer resolution.
    *   Configure logging via `tracing_subscriber` (directing logs to stderr to keep stdout pure for MCP JSON-RPC).
    *   Initialize Tokio multi-threaded runtime.

#### 11. `crates/pico429/src/transport/device.rs` (Create)
*   **Type / Scope**: Host OS Serial Communication Layer
*   **Proposed Changes**:
    *   Manage OS serial port handle (`serialport`) in dedicated non-blocking thread or IOCP async worker.
    *   Implement USB RX continuous stream scanner with 1-byte sliding window (`find_frame_in_stream`).
    *   Implement 64-byte batch packing with 250 µs timer for USB TX.
    *   Handle automatic serial reconnection upon USB disconnect / re-enumeration.

#### 12. `crates/pico429/src/transport/scheduler.rs` (Create)
*   **Type / Scope**: Dual-Lane Priority Scheduler & Sliding Window Engine
*   **Proposed Changes**:
    *   Lane 0 (Expedited Control Plane Lane): High-priority unbounded channel for `CMD_REQ` (`ping`, `config`, `status`, `ABORT_TX`), bypassing credit checks and preempting stream packets.
    *   Lane 1 (Streaming Data Lane): Per-channel credit semaphores (`tokio::sync::Semaphore`) preventing cross-channel head-of-line blocking.
    *   Relative baseline latching: Upon receiving bootstrap `STREAM_CREDIT`, latch `BaselineConsumed[ch] = ReportedConsumed[ch]`.
    *   Relative consumption tracking: $\text{InFlight}[ch] = \text{SentCount}[ch] - (\text{ReportedConsumed}[ch].\text{wrapping\_sub}(\text{BaselineConsumed}[ch]))$.
    *   Baud-rate aware dynamic credit ceilings: 128 words for 100 kbps, 32 words for 12.5 kbps.

#### 13. `crates/pico429/src/telemetry/buffer.rs` (Create)
*   **Type / Scope**: In-Memory Telemetry Ring Buffers
*   **Proposed Changes**:
    *   Per-channel bounded `VecDeque<InboundArincFrame>` (depth 32,768).
    *   Track monotonic dropped frame counters, sequence gap counters, and parity error metrics.
    *   Expose filtered retrieval API (`fetch_telemetry(channel, limit, label_filter)`).

#### 14. `crates/pico429/src/telemetry/cvt.rs` (Create)
*   **Type / Scope**: Host Current Value Table (CVT)
*   **Proposed Changes**:
    *   Maintain `[AtomicU32; 256]` (latest raw word) and `[AtomicU32; 256]` (latest timestamp $\mu\text{s}$) per channel (~24 KB total).
    *   Provides $O(1)$ deterministic state inspection for AI agents, independent of agent turn latency or ring buffer overwrites.

#### 15. `crates/pico429/src/mcp/server.rs` (Create)
*   **Type / Scope**: Native MCP Server Implementation
*   **Proposed Changes**:
    *   Implement `Pico429McpServer` with `#[tool_router(server_handler)]`.
    *   Implement declared tools: `pico429_get_status`, `pico429_configure_channels`, `pico429_send_word`, `pico429_stream_send` (with synchronous completion default and `StreamJobManager` async fallback), `pico429_stream_status`, `pico429_abort_stream`, `pico429_fetch_telemetry`, `pico429_get_latest_labels`, and `pico429_run_bit`.
    *   Run server loop over `rmcp::transport::stdio::StdioTransport`.

#### 16. `crates/pico429/tests/streaming_tests.rs` (Create)
*   **Type / Scope**: Host Integration Tests
*   **Proposed Changes**:
    *   Verify multi-channel streaming concurrency and independent per-channel semaphore throttling.
    *   Verify credit underflow protection, baseline latching on reconnect, and `ABORT_TX` queue purge.
    *   Stress-test Current Value Table updates against high-rate simulated telemetry.

#### 17. `tools/` (Deprecate / Archive)
*   **Type / Scope**: Legacy Python Prototype Tools
*   **Proposed Changes**:
    *   Deprecate `tools/pico429.py` and `tools/arinc429_codec.py`.
    *   Add deprecation banner in `tools/README.md` pointing users to the new unified Rust binary (`pico429`).

#### 18. `docs/USER_GUIDE.md` (Modify)
*   **Type / Scope**: Operator & User Documentation
*   **Proposed Changes**:
    *   Update quickstart instructions from Python virtual environment scripts to standalone `cargo run --bin pico429 -- ...` or `pico429.exe`.
    *   Document new streaming CLI options and MCP server configuration for Claude Code, Cursor, and Antigravity.

### Verification Plan

*   **Compilation/Build**:
    *   Verify protocol core: `cargo build --package arinc429-core`
    *   Verify host binary: `cargo build --package pico429 --release`
    *   Verify firmware: `cargo build --package pico-429-firmware --target thumbv8m.main-none-eabihf --release`
*   **Unit & Integration Tests**:
    *   `cargo test --package arinc429-core` (ensuring 100% protocol and frame codec coverage).
    *   `cargo test --package pico429` (testing priority scheduler, sliding-window credit math, ring buffers, and CVT).
*   **Static Analysis & Linting (Partitioned by Target)**:
    *   Host & Core: `cargo clippy --package arinc429-core --package pico429 --all-targets -- -D warnings`
    *   Firmware: `cargo clippy --package pico-429-firmware --target thumbv8m.main-none-eabihf --all-targets -- -D warnings`
    *   Format: `cargo fmt --all --check`
*   **Integration & Stress Testing**:
    *   **Hardware Loopback BIT**: Run `pico429 bit --tx 0 --rx 1 --count 10000` verifying 0 dropped words and 100% parity accuracy.
    *   **12-Channel Full-Rate Streaming**: Run `pico429 stream --channels 0..5 --rate 2778 --count 100000` verifying continuous 533.3 KB/s transfer with CPU utilization < 0.5% and zero starvation gaps.
    *   **MCP Protocol Compliance**: Connect Claude Code / Antigravity via stdio and execute automated BIT diagnostic and channel configuration.

--------------------------------------------------------------------------------

*Drafted with EGM*

