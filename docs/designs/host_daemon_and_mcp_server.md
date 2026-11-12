# Pico-429 High-Throughput Host Daemon & MCP Server

*   **Author**: lihan
*   **Status**: Approved
*   **Date**: 2026-11-11

--------------------------------------------------------------------------------

## Overview

High-throughput streaming host service and AI agent integration interface for the Pico-429 multi-channel ARINC 429 transceiver platform. The service provides a persistent host daemon (`pico429d`) to maintain continuous USB connection, execute pipelined bulk transfers for 12-channel full-load traffic (533 KB/s), and expose structured Model Context Protocol (MCP) endpoints for autonomous agent operation.

## The Problem

The initial host software in `tools/pico429/` was designed for low-frequency command-line debugging. It exhibits three structural limitations that prevent sustained multi-channel operation:

1. **Synchronous Stop-and-Wait Transmission Bottleneck**:
   Sending an ARINC 429 word requires a synchronous `CMD_REQ (TX_RAW_WORD)` followed by waiting for a `CMD_ACK` frame. Over Windows USB CDC (`usbser.sys`), each round-trip incurs a minimum scheduling latency of ~0.5 ms to 1.0 ms. This physical stop-and-wait latency caps maximum TX throughput to 1,000–2,000 words/s across all channels combined. In contrast, 6 high-speed TX channels running at 100 kbps require 16,667 words/s (2,778 words/s/channel), resulting in immediate transmitter starvation and buffer overrun.

2. **Transient Connection Lifecycle**:
   Every CLI invocation opens, configures, and closes the serial port. On Windows, closing CDC ports while asynchronous IN/OUT transfers are pending leads to USB endpoint stalls, write timeouts (Win32 Error 121), and loss of incoming telemetry frames while the port is closed.

3. **Lack of Autonomous Agent Interface**:
   No machine-readable RPC or agent integration layer exists. AI agents interacting with the system must spawn ad-hoc Python subshell processes with raw string parsing, which lacks type validation, schema introspection, backpressure management, and connection state persistence.

## Goals

*   **Pipelined Full-Load Streaming**:
    Sustain full-rate bidirectional ARINC 429 traffic on all 12 channels simultaneously (up to 33,336 words/s total, equivalent to 533.3 KB/s USB payload) without frame drops or host thread starvation.
*   **Persistent Host Daemon**:
    Maintain a single resident service holding the USB handle open, managing thread-safe transmit and receive bounded buffers, and eliminating port reopening overhead.
*   **64-Byte USB Packet Packing**:
    Batch transmission frames into 64-byte USB OUT endpoint packets (4 frames $\times$ 16 bytes = 64 bytes) to maximize USB 1.1 Full Speed bus utilization.
*   **Cumulative Sliding-Window Flow Control**:
    Replace per-word synchronous ACK with a windowed credit/watermark flow control mechanism between firmware and host, allowing non-blocking pipelined streaming while preventing buffer overflows.
*   **Dual-Lane Priority Scheduling**:
    Maintain an expedited command lane bypassing bulk streaming flow control to guarantee < 1.0 ms latency for interactive commands under heavy streaming backpressure.
*   **Model Context Protocol (MCP) Server**:
    Provide a standardized MCP server over stdio exposing tools (`pico429_send_word`, `pico429_stream_send`, `pico429_fetch_telemetry`, `pico429_configure_channels`, `pico429_run_bit`) with JSON Schema validation for AI agents.

## Non-Goals

*   **Desktop Graphical User Interface (GUI)**:
    No dedicated desktop UI (Qt, Electron, Tkinter) will be implemented in this phase. Existing CLI tools will be re-routed to talk to the resident daemon or client library.
*   **Alternative Physical Transports**:
    Wi-Fi, Ethernet, and BLE transports are out of scope. The transport remains strictly USB Full Speed CDC-ACM.
*   **Hardware Schedule Injection / Custom Arbitrary Waveform Generation**:
    Complex cyclic ARINC 429 schedule tables with sub-microsecond jitter guarantees will remain in firmware PIO timers rather than being scheduled frame-by-frame from Python userspace.

## Open Questions

*   **Question 1 (Daemon IPC Implementation)**:
    Resolved: Use Python standard library `multiprocessing.connection` (Windows Named Pipes `\\.\pipe\pico429_daemon`, Unix Domain Sockets `/tmp/pico429.sock`). Eliminates external dependencies and custom framing logic while providing authenticated local message exchange.
*   **Question 2 (Firmware Buffer Watermark Protocol)**:
    Resolved: Standard 16-byte `STREAM_CREDIT` frame emitted per-channel whenever the channel TX FIFO drains below 50% capacity, plus an initial credit grant frame emitted upon USB CDC connection.

## Critical User Journeys

### CUJ 1: 12-Channel Full-Rate Streaming Test
*   **Given**: Pico-429 hardware connected over USB with 6 channels configured as TX (100 kbps) and 6 channels looped back to RX (100 kbps). Daemon running in the background.
*   **When**: Host application streams 100,000 words uniformly distributed across the 6 TX channels at 16,667 words/s (33,334 words/s total traffic).
*   **Then**: All 100,000 words are transmitted without write timeouts; all 100,000 loopback words are received and decoded; frame loss is 0; USB host-to-bus latency remains under 2.0 ms.

### CUJ 2: AI Agent Autonomous Bus Diagnostics via MCP
*   **Given**: Pico-429 daemon running with MCP server exposed on stdio. AI Agent connected via standard MCP client.
*   **When**: Agent calls `pico429_run_bit(tx_channel=0, rx_channel=1)` to verify loopback integrity, followed by `pico429_send_word(channel=0, label=203, data=12345)`.
*   **Then**: Agent receives structured JSON response confirming pass/fail status, sequence counters, and decoded telemetry without invoking raw terminal scripts or handling serial framing errors.

--------------------------------------------------------------------------------

## Technical Plan

### 1. Multi-Tier System Architecture

The refactored host stack decouples serial port ownership, protocol stream batching, and external interfaces into three distinct layers:

```text
+-----------------------------------------------------------------------------+
|                            Application Layer                                |
|   +--------------------------+         +--------------------------------+   |
|   |  AI Agent / LLM Host     |         |  Existing CLI & Scripts        |   |
|   |  (Claude Code / Cursor)  |         |  (tools/pico429/cli/*.py)      |   |
|   +------------+-------------+         +---------------+----------------+   |
|                | stdio (JSON-RPC)                      | IPC Client         |
|                v                                       |                    |
|   +--------------------------+                         |                    |
|   |  pico429-mcp-server      |                         |                    |
|   |  (Ephemeral Adapter)     |                         |                    |
|   +------------+-------------+                         |                    |
|                | IPC Client                            |                    |
+----------------|---------------------------------------|--------------------+
                 +-------------------+-------------------+
                                     |
                                     v Local IPC (multiprocessing.connection)
                                     | Named Pipe: \\.\pipe\pico429_daemon
+-----------------------------------------------------------------------------+
|                       Host Resident Daemon (pico429d)                       |
|   +---------------------------------------------------------------------+   |
|   |                        Daemon Core Engine                           |   |
|   |   - Channel State & Configuration Cache (Reconnection Replay)      |   |
|   |   - Cumulative Sliding-Window Credit Flow Controller                |   |
|   |   - Telemetry Pub-Sub Router (collections.deque per channel)        |   |
|   +----------------------------------+----------------------------------+   |
|                                      |                                      |
|                                      v                                      |
|   +---------------------------------------------------------------------+   |
|   |                       Transport Worker Thread                       |   |
|   |   - Lane 0: Expedited Command Queue (CMD_REQ, bypasses stream credit|   |
|   |   - Lane 1: Stream Queue (STREAM_TX_DATA, 64B batching, <=250us flush)| 
|   |   - Inbound 64-byte Frame Parser & C-Accelerated CRC16 Checksum     |   |
|   +----------------------------------+----------------------------------+   |
+--------------------------------------|--------------------------------------+
                                       | USB 1.1 Full-Speed Bulk Packets
                                       | (64 bytes / transaction)
                                       v
+-----------------------------------------------------------------------------+
|                           Pico-429 RP2350 Firmware                          |
|   +---------------------------------------------------------------------+   |
|   | USB CDC-ACM Endpoint Driver (embassy-usb)                           |   |
|   |   - OUT Endpoint (64B) -> AlignedStreamBuf (512B)                   |   |
|   |   - CHANNEL_TX_QUEUES (64 words / channel, 3 KB SRAM)               |   |
|   |   - Watermark Monitor  -> Pushes STREAM_CREDIT to USB_CMD_RESP_QUEUE|   |
|   |   - IN Endpoint (64B)  <- Multiplexed CMD_ACK / STREAM_CREDIT / DATA|   |
|   +----------------------------------+----------------------------------+   |
|                                      |                                      |
|                                      v                                      |
|   +---------------------------------------------------------------------+   |
|   | 12 x Hardware State Machines (RP2350 PIO Core 0 & Core 1)           |   |
|   |   - 100 kbps Bipolar RZ Transmission & Reception                    |   |
|   +---------------------------------------------------------------------+   |
+-----------------------------------------------------------------------------+
```

### 2. Streaming Protocol Evolution

To break the 1,000 wps synchronous stop-and-wait ceiling, the 16-byte binary protocol introduces high-throughput streaming frames while retaining all existing command and telemetry frames:

#### 2.1 Baseline & Streaming Frame Types
*   `0x01` (`ARINC_DATA`): Existing downlink telemetry frame (Rx timestamp + raw ARINC word).
*   `0x02` (`CMD_ACK`): Existing command response.
*   `0x03` (`ASYNC_EVENT`): Existing hardware / buffer exception event.
*   `0x04` (`CMD_REQ`): Existing synchronous command request.
*   `0x05` (`STREAM_TX_DATA`): Asynchronous outbound data word.
    *   Byte 0–1: Magic `0xA429` (little-endian)
    *   Byte 2: `(0x05 << 4) | (channel & 0x0F)`
    *   Byte 3: `flags` (bit 0: odd parity check request)
    *   Byte 4–5: `seq_num` (u16 monotonic sequence tag for gap auditing)
    *   Byte 6–9: `payload_0` (u32, 32-bit ARINC 429 raw word)
    *   Byte 10–13: `payload_1` (u32, host timestamp or sub-channel sequence)
    *   Byte 14–15: `crc16` (CRC-16-CCITT)
    *   **Behavior**: Firmware directly enqueues word to channel TX FIFO without generating individual `CMD_ACK`.
*   `0x06` (`STREAM_CREDIT`): Asynchronous firmware flow control credit update.
    *   Byte 0–1: Magic `0xA429` (little-endian)
    *   Byte 2: `(0x06 << 4) | (channel & 0x0F)` (channel 0–11)
    *   Byte 3: `flags` (reserved, set to 0x00)
    *   Byte 4–5: `seq_num` (monotonic credit frame counter)
    *   Byte 6–9: `payload_0` (u32, available FIFO slots in firmware for this channel)
    *   Byte 10–13: `payload_1` (u32, cumulative 32-bit counter of ARINC words consumed by PIO)
    *   Byte 14–15: `crc16` (CRC-16-CCITT)

#### 2.2 Endpoint Alignment & Dual-Lane Queue Scheduling
*   **Dual-Lane Scheduler**:
    *   **Lane 0 (High Priority)**: `CMD_REQ` frames bypass streaming credit gates and flush immediately as short USB packets (16 or 32 bytes). This guarantees < 1.0 ms latency for commands like `pico429_get_status` and channel reconfigurations during full-rate streaming.
    *   **Lane 1 (Best Effort)**: `STREAM_TX_DATA` frames accumulate up to four 16-byte frames ($4 \times 16 = 64$ bytes) into a single OS write call to match the RP2350 USB hardware endpoint buffer size. If fewer than 4 frames are queued, an adaptive micro-timer ($\le 250\ \mu\text{s}$) flushes partial packets to bound packing delay.
*   **Firmware RX Unpacker**: In `usb_cdc_rx_task`, incoming packets are read into an expanded `AlignedStreamBuf([u8; 512])`. Frames with `STREAM_TX_DATA` are parsed with zero-copy stream slicing and enqueued directly to `CHANNEL_TX_QUEUES[ch]`.

#### 2.3 Cumulative Sliding-Window Backpressure Mechanism
```text
  Host Daemon (pico429d)                              Firmware RP2350
       |                                                    |
       |<-- Initial STREAM_CREDIT (ch=0, slots=64, consumed=0) (on USB connect)
       |                                                    |
       |=== Bulk Send 64-byte packet (4 words to ch 0) =====>|  [FIFO: 4/64]
       |=== Bulk Send 64-byte packet (4 words to ch 0) =====>|  [FIFO: 8/64]
       |    InFlight = Sent(8) - Consumed(0) = 8            |
       |    UsableCredit = 64 - InFlight = 56               |  [PIO drains words]
       |                                                    |  [FIFO drops to 32/64]
       |<-- STREAM_CREDIT (ch=0, slots=32, consumed=32) ----|  [Pushed to CMD_QUEUE]
       |    InFlight = Sent(8) - Consumed(32) = 0           |
       |    UsableCredit = min(64 - 0, 32) = 32             |
```

*   **Self-Healing Properties**: Because `payload_1` reports cumulative consumed words, dropped or delayed credit frames never leak credits or cause window desynchronization.
*   **Firmware Inter-Task Signaling**: When a PIO driver task on Core 0/1 drains `CHANNEL_TX_QUEUES[ch]` below 50% capacity (32 words), it constructs `BinaryFrame::new_stream_credit(ch, 32, consumed_count)` and enqueues it to `USB_CMD_RESP_QUEUE`. `usb_cdc_tx_task` receives it and transmits it to the host immediately.

### 3. Host Resident Daemon (`pico429d`) Architecture

The daemon operates as an autonomous background process:

*   **Dedicated Transport Thread**:
    Runs an uninterrupted non-blocking serial I/O loop using `pyserial`. Ingests raw USB chunks into memory and uses standard library C-accelerated `binascii.crc_hqx` for CRC16 calculation, guaranteeing < 5% host CPU load under 533 KB/s.
*   **Thread-Safe Bounded Telemetry Ring Buffers**:
    Maintains circular buffers using standard library `collections.deque(maxlen=32768)` for each channel. Each channel tracks a monotonic `sequence_gap_counter` and `dropped_words_counter` to flag buffer wrap-around explicitly.
*   **Standard Library Local IPC**:
    Exposes a local IPC endpoint using Python's `multiprocessing.connection`:
    *   Windows: Named Pipe (`\\.\pipe\pico429_daemon`).
    *   Linux / macOS: Unix Domain Socket (`/tmp/pico429.sock`).
*   **Reconnection State Machine**:
    Upon USB cable re-enumeration, `pico429d`:
    1. Reopens the COM port handle.
    2. Replays cached channel operating modes (`SET_CHANNEL_CFG`).
    3. Replays active label filters (`SET_FILTER`).
    4. Resets credit counters and waits for the initial `STREAM_CREDIT` frame before resuming transmission.

### 4. Model Context Protocol (MCP) Server

The MCP server runs as a lightweight stdio adapter process (`pico429-mcp-server`), spawned directly by AI coding agents (Claude Code, Cursor, Antigravity). It connects to `pico429d` via local IPC.

#### 4.1 MCP Tools Specification

| Tool Name | Parameters | Description |
| :--- | :--- | :--- |
| `pico429_get_status` | None | Returns device connection state, active channel directions, PIO speed configs, and FIFO telemetry stats. |
| `pico429_configure_channels` | `channels`: list[dict] | Configures channel operating modes in batch (`channel`, `direction`, `speed`). |
| `pico429_send_word` | `channel`: int, `label`: int, `data`: int, `sdi`: int = 0, `ssm`: int = 0, `parity`: str = "odd" | Transmits a single validated ARINC 429 word with < 1.0 ms latency. |
| `pico429_stream_send` | `channel`: int, `words`: list[int], `rate_hz`: int = 1000 | Injects a sequence of 32-bit ARINC 429 words using credit-paced batching. Returns `{job_id, status: "started"}`. |
| `pico429_stream_status` | `job_id`: str | Returns progress, words sent, and active backpressure status of a streaming job. |
| `pico429_abort_stream` | `job_id`: str | Cancels an ongoing stream transmission and flushes unsent words. |
| `pico429_fetch_telemetry` | `channel`: int, `limit`: int = 100, `label_filter`: int = None | Queries recent decoded telemetry from the channel ring buffer. |
| `pico429_run_bit` | `tx_channel`: int, `rx_channel`: int, `count`: int = 1000 | Executes an autonomous hardware loopback Built-In Test in daemon memory, returning structured pass/fail metrics. |

#### 4.2 Structured Error Handling & Schema Validation
All MCP tools enforce strict JSON Schema validation. Bus errors return structured diagnostic payloads rather than generic process exits, allowing AI agents to diagnose and correct communication parameters programmatically.

--------------------------------------------------------------------------------

## Alternatives Considered

### Alternative A: Stateless CLI with Process-Level Serial Port Locking

Keep the current stateless CLI model, but add file-based or mutex port locking and shorten timeouts, attempting to increase burst rate without a resident daemon.

*   **Pros**:
    *   Simple architecture with zero daemon lifecycle management.
    *   No inter-process communication (IPC) layer required.
*   **Cons**:
    *   Fails to solve the root problem: Windows serial driver (`usbser.sys`) initialization and teardown cost (30–100 ms per execution).
    *   Incoming telemetry arriving between CLI invocations is discarded or dropped by the host OS buffer when DTR is toggled.
    *   Cannot sustain continuous high-rate streaming (16,667 words/s) across multiple channels.
*   **Reason for Rejection**: Fails to meet the 533 KB/s continuous streaming requirement and causes Windows CDC endpoint stalls upon repeated open/close.

### Alternative B: Standalone REST / HTTP Daemon Instead of MCP

Implement a resident daemon running a local HTTP web server (e.g. FastAPI / Flask) exposing JSON REST endpoints (`/api/send`, `/api/telemetry`).

*   **Pros**:
    *   Universal web tooling compatibility; testable via `curl` or browser.
    *   Well-understood API paradigms for standard web developers.
*   **Cons**:
    *   Significant per-request latency and CPU serialization overhead for high-frequency ARINC 429 streaming.
    *   Requires AI agents to parse raw HTTP responses or write boilerplate wrapper code, lacking standardized agent tool discovery and context integration.
    *   Does not leverage standard IDE agent integrations (Claude Code, Cursor, Antigravity) which natively adopt the Model Context Protocol.
*   **Reason for Rejection**: HTTP overhead is excessive for microsecond-scale bus operations, and requires an extra adapter layer to make it accessible to AI agents compared to native MCP.

### Alternative C: Firmware-Side Autonomous Schedule Generator (Zero Host Streaming)

Offload full-load stream generation entirely into firmware flash/RAM; host uploads static cyclic schedule tables (e.g. 100 words at 50 Hz), and RP2350 autonomously loops transmission without continuous host USB writes.

*   **Pros**:
    *   Completely immune to host USB scheduling latency and OS jitter.
    *   Zero host CPU overhead during transmission.
*   **Cons**:
    *   Sacrifices dynamic closed-loop testing capability (e.g., Hardware-in-the-Loop simulation where received words influence transmitted words in real time).
    *   Increases firmware complexity and consumes RP2350's limited 520 KB SRAM.
    *   Prevents AI agents from interactively manipulating word payloads, injecting fault words, or testing protocol edge cases without repeatedly recompiling or re-uploading schedules.
*   **Reason for Rejection**: Sacrifices interactive testing and closed-loop agent control in favor of static load generation.

--------------------------------------------------------------------------------

## Non-Functional Requirements

### 1. Performance & Latency
*   **Throughput**: Must sustain continuous bidirectional 533 KB/s USB payload transfer (33,334 words/s) without process CPU utilization exceeding 5% on modern multicore x86/ARM host machines.
*   **Streaming Latency**: Host-to-bus queue latency for batched frames must remain under 2.0 ms during full-rate streaming.
*   **Single-Word Interactive Latency**: Direct single-word transmission via the expedited command queue must complete in < 1.0 ms.

### 2. Reliability & Fault Recovery
*   **USB Disconnect Resilience**: If the USB cable is unplugged, the daemon must transition to a disconnected state, preserve channel configuration parameters, and automatically rebind upon re-enumeration without crashing.
*   **Buffer Bounds**: Telemetry queues must enforce bounded capacity policies (ring buffer overwrite of oldest data with explicit gap tracking) to prevent unbounded memory growth.

--------------------------------------------------------------------------------

## Detailed Implementation Plan

### Target Files to Create/Modify

#### 1. `crates/arinc429-core/src/protocol.rs` (Modify)
*   **Type / Scope**: Firmware Core Protocol Definition (Rust `no_std`)
*   **Proposed Changes**:
    *   Preserve `frame_types::ARINC_DATA = 0x01` and `frame_types::CMD_REQ = 0x04`.
    *   Add `frame_types::STREAM_TX_DATA = 0x05` and `frame_types::STREAM_CREDIT = 0x06`.
    *   Add constructor helper `BinaryFrame::new_stream_tx(channel: u8, seq: u16, word: u32, tag: u32) -> Self`.
    *   Add constructor helper `BinaryFrame::new_stream_credit(channel: u8, available_credits: u32, processed_count: u32) -> Self`.
*   **Rationale**: Defines the 16-byte binary protocol wire format for high-throughput streaming and cumulative backpressure.

#### 2. `crates/firmware/src/channel.rs` (Modify)
*   **Type / Scope**: Hardware Channel Management (Rust `no_std`)
*   **Proposed Changes**:
    *   Expand `CHANNEL_TX_QUEUES` depth from 16 to 64 words: `Channel<CriticalSectionRawMutex, u32, 64>`. Total SRAM cost is ~3 KB across 12 channels.
    *   Expose non-blocking buffer occupancy metrics (`channel_tx_available_space(ch: u8) -> usize`).
    *   In the PIO consumption loop, trigger credit emission when queue occupancy drops below 32 words.
*   **Rationale**: Provides a 23 ms buffer at 100 kbps to bridge Windows USB 1 ms scheduling jitter and prevent transmitter starvation.

#### 3. `crates/firmware/src/usb_service.rs` (Modify)
*   **Type / Scope**: Embedded USB CDC Service (Rust `embassy-rp`)
*   **Proposed Changes**:
    *   Expand `AlignedStreamBuf` in `usb_cdc_rx_task` from 128 to 512 bytes.
    *   In `usb_cdc_rx_task`, intercept `STREAM_TX_DATA` in the primary packet scan loop. Push raw words directly to target channel's TX queue via `send_to_channel_tx(ch, word)` without generating `CMD_ACK`.
    *   In `usb_cdc_tx_task`, emit initial `STREAM_CREDIT` upon connection, and forward credit frames from `USB_CMD_RESP_QUEUE` to the host.
*   **Rationale**: Eliminates per-word ACK turnaround overhead on TX data and provides closed-loop credit feedback to prevent buffer overflow under 533 KB/s load.

#### 4. `tools/arinc429_codec.py` (Modify)
*   **Type / Scope**: Python Protocol Serialization & Codec
*   **Proposed Changes**:
    *   Add `STREAM_TX_DATA = 0x05` and `STREAM_CREDIT = 0x06` to `FrameType`.
    *   Update `crc16_ccitt(data)` to use standard library `binascii.crc_hqx(data, 0xFFFF)` for compiled C performance.
    *   Add batch encoding helper `encode_frames_batch(frames: Sequence[BinaryFrame]) -> bytes` ensuring 64-byte alignment.
*   **Rationale**: Eliminates pure Python CRC CPU bottlenecks and provides host-side serialization for streaming frames.

#### 5. `tools/pico429/daemon/transport_worker.py` (Create)
*   **Type / Scope**: Dedicated Transport Thread (Python Threading)
*   **Proposed Changes**:
    *   Manage persistent serial port handle with non-blocking read/write loop.
    *   Implement dual-lane scheduler: Lane 0 (Expedited Command Queue for `CMD_REQ`) and Lane 1 (Credit-gated 64-byte streaming queue with 250 µs flush timer).
    *   Implement cumulative sliding window credit tracker.
    *   Maintain per-channel telemetry buffers using standard library `collections.deque(maxlen=32768)` with sequence gap counters.
*   **Rationale**: Isolates serial I/O, credit tracking, and batching in a dedicated thread to guarantee responsive dispatch.

#### 6. `tools/pico429/daemon/ipc.py` (Create)
*   **Type / Scope**: Local Inter-Process Communication (Python)
*   **Proposed Changes**:
    *   Implement `IPCServer` and `IPCClient` using standard library `multiprocessing.connection` (`Listener` and `Client`).
    *   Bind Named Pipe (`\\.\pipe\pico429_daemon`) on Windows and Unix Domain Socket (`/tmp/pico429.sock`) on Linux/macOS.
    *   Exchange Python dict messages for control commands and raw memory slices for telemetry.
*   **Rationale**: Provides authenticated, framed local IPC with zero external dependencies and sub-millisecond latency.

#### 7. `tools/pico429/daemon/service.py` (Create)
*   **Type / Scope**: Daemon Lifecycle Manager (Python)
*   **Proposed Changes**:
    *   Handle daemon startup, device auto-discovery via `find_pico_port`, clean shutdown, and reconnection recovery.
    *   On USB rebind, replay cached channel configurations and label filters before releasing outbound streaming queues.
    *   Provide entrypoint command `pico429 daemon run`.
*   **Rationale**: Coordinates worker threads, IPC server, and hardware state recovery.

#### 8. `tools/pico429/mcp/server.py` (Create)
*   **Type / Scope**: Model Context Protocol Server (Python)
*   **Proposed Changes**:
    *   Implement standard MCP server over `stdio` using official Python MCP SDK or fast JSON-RPC adapter.
    *   Register tools: `pico429_get_status`, `pico429_configure_channels`, `pico429_send_word`, `pico429_stream_send`, `pico429_stream_status`, `pico429_abort_stream`, `pico429_fetch_telemetry`, `pico429_run_bit`.
    *   Proxy all tool calls to `pico429d` via `IPCClient`.
*   **Rationale**: Empowers AI agents to interact with hardware through structured tool calling with JSON Schema verification.

#### 9. `tools/pico429/mcp/__init__.py` (Create)
*   **Type / Scope**: Package Init
*   **Proposed Changes**: Expose `run_mcp_server()` entrypoint.
*   **Rationale**: Standard package initialization.

#### 10. `tools/pico429/daemon/__init__.py` (Create)
*   **Type / Scope**: Package Init
*   **Proposed Changes**: Export `DaemonService`, `IPCClient`.
*   **Rationale**: Standard package initialization.

#### 11. `tools/pico429/client.py` (Modify)
*   **Type / Scope**: High-Level Client API (Python)
*   **Proposed Changes**:
    *   Route client operations through local daemon via `IPCClient` when active; fallback to direct serial connection when `--direct` is specified.
    *   Add high-throughput streaming methods: `stream_send(channel, words, rate_hz)` and `subscribe_telemetry(channel, callback)`.
*   **Rationale**: Maintains backward compatibility for existing scripts while accelerating operations through the daemon.

#### 12. `tools/pico429/cli/parser.py` & `tools/pico429/cli/handlers.py` (Modify)
*   **Type / Scope**: Command-Line Interface (Python)
*   **Proposed Changes**:
    *   Add `daemon` subcommands: `pico429 daemon run`, `pico429 daemon status`.
    *   Add `mcp` subcommand: `pico429 mcp serve`.
    *   Add `stream` subcommand: `pico429 stream send --ch <ch> --rate <hz> <words>`.
*   **Rationale**: Expose daemon control, streaming injection, and MCP server startup via CLI commands.

#### 13. `docs/USER_GUIDE.md` (Modify)
*   **Type / Scope**: User Documentation
*   **Proposed Changes**:
    *   Document the persistent daemon architecture, installation, and management commands.
    *   Add instructions for configuring the MCP server in Claude Code, Cursor, and Antigravity.
    *   Add high-throughput streaming examples and benchmarks.
*   **Rationale**: Keeps user documentation synchronized with architectural capabilities.

### Verification Plan

*   **Compilation & Firmware Verification**:
    *   `cargo check --target thumbv8m.main-none-eabihf --package pico-429-firmware`
    *   `cargo test --package arinc429-core`
*   **Host Code Verification & Quality Gates**:
    *   `ruff check tools/ tests/`
    *   `ruff format --check tools/ tests/`
    *   `ty check tools/ tests/` (or `mypy tools/ tests/`)
    *   `uv run python -m unittest discover tests`
*   **High-Throughput 12-Channel Full-Load E2E Verification**:
    *   Configure 6 TX channels and 6 RX channels (100 kbps) with hardware jumper loopbacks.
    *   Stream 100,000 words uniformly across the 6 channels at 16,667 words/s (33,334 words/s aggregate traffic = 533.3 KB/s payload).
    *   Verify 0 dropped words, 0 sequence discrepancies, per-batch round-trip latency < 5 ms, and process CPU utilization < 5%.
*   **MCP Agent Integration Verification**:
    *   Start MCP server using `pico429 mcp serve`.
    *   Execute simulated MCP client calls against `pico429_run_bit`, `pico429_send_word`, and `pico429_fetch_telemetry`.
    *   Confirm all JSON payloads validate against declared schemas.

--------------------------------------------------------------------------------

<!-- Provenance: Finalized following EGM Goldfish Review Protocol (Stages 5–9). -->

*Finalized with EGM*
