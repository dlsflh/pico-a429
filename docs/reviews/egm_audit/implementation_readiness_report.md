# Stage 9 Report: Implementation Readiness Review

*   **Target Document**: `docs/designs/pure_rust_host_and_mcp_server.md`
*   **Target Range**: DetailedRange (lines 378..506)
*   **Reviewers**: `egm-implementation-readiness` (Pragmatic Tech Lead), `egm-codebase-details` (Pedantic Codebase Details Auditor), `egm-style` (Technical Copy Editor)
*   **Status**: Complete

---

## Executive Summary

The implementation readiness review evaluated the execution instructions in `DetailedRange` (lines 378..506) for precision, completeness, and consistency against the live codebase.

While the high-level plan correctly identifies the major modules, **three critical blockers** prevent immediate implementation:
1. **Broken Verification Commands**: The plan references `cargo build --package firmware`, but in `crates/firmware/Cargo.toml` the package is named `pico-429-firmware`. Furthermore, `cargo clippy --workspace --all-targets` fails because bare-metal and host targets cannot be evaluated in a single unpartitioned pass.
2. **Missing Driver Hook for Word Popping**: Words are popped from `CHANNEL_TX_QUEUES` and pushed to the PIO FIFO inside `crates/firmware/src/driver/device.rs`, which was omitted from the list of files to modify. Without modifying `device.rs`, consumption counters and credit watermarks cannot be triggered.
3. **Missing Word Codec Module in Task List**: `crates/arinc429-core/src/word.rs` currently only provides raw word wrappers and lacks `encode_word`, SDI/SSM packing, and parity application required by the MCP server and CLI.

Additionally, several major discrepancies must be addressed, including bootstrap credit broadcasting on connection, concurrency topology for multi-channel streaming without cross-channel blocking, host `src/lib.rs` exposure for unit tests, Windows 250 µs timer resolution initialization (`timeBeginPeriod`), and `rmcp` SDK feature flag corrections.

---

## Findings Matrix

| ID | Priority | Dimension | Core Finding | Actionable Remediation |
| :--- | :--- | :--- | :--- | :--- |
| **IR-01** | 🔴 **Blocker** | Build / Verification | `cargo build --package firmware` fails (`pico-429-firmware`); workspace clippy fails across mixed host/Cortex-M targets. | Update command to `cargo build --package pico-429-firmware`; partition clippy into host and embedded target runs. |
| **IR-02** | 🔴 **Blocker** | Missing Target File | Transmit words are popped in `crates/firmware/src/driver/device.rs`, which is omitted from target files. | Add `driver/device.rs` to target files; call `record_tx_word_consumed(ch)` after `sm.tx().wait_push(raw_word).await`. |
| **IR-03** | 🔴 **Blocker** | Missing Target File & API | `crates/arinc429-core/src/word.rs` lacks `encode_word` / `apply_parity` and is omitted from target files. | Add `crates/arinc429-core/src/word.rs` to target files; implement `encode_word` with label reversal, SDI, SSM, and parity. |
| **IR-04** | 🟡 **Major** | Bootstrap Credits | Firmware task list omits initial `STREAM_CREDIT` broadcast upon USB connection, causing host streaming to stall with credit 0. | Add task in `usb_service.rs` to broadcast bootstrap credits (slots = 128) upon host enumeration. |
| **IR-05** | 🟡 **Major** | Concurrency / Queueing | Single shared Lane 1 MPSC channel causes backpressured channels to block words destined for unblocked channels. | Provide per-channel credit semaphores or 12 dedicated MPSC channels with round-robin polling in `scheduler.rs`. |
| **IR-06** | 🟡 **Major** | Module Structure / Tests | Host binary files declared in `src/main.rs` cannot be imported by integration tests without `src/lib.rs`. | Add `crates/pico429/src/lib.rs` and specify integration test files in target files list. |
| **IR-07** | 🟡 **Major** | OS / Timer Resolution | Default Windows timer resolution (15.6 ms) throttles 250 µs batch timer by $60\times$, collapsing USB streaming throughput. | Mandate calling `timeBeginPeriod(1)` at host startup or using an adaptive hybrid spin-wait for sub-ms timeouts. |
| **IR-08** | 🟡 **Major** | Async Job Architecture | MCP streaming tools rely on `job_id`, but `JobManager` data structures, cancellation tokens, and UUIDs are unspecified. | Add `tokio-util` and `uuid` to dependencies; explicitly specify `StreamJobManager` tracking state and cancellation. |
| **IR-09** | 🟡 **Major** | Legacy File References | File paths `tools/pico429/pico429_cli.py`, `tools/pico429_mcp_server.py`, and `tools/pico429/README.md` do not exist. | Update paths to actual repository files: `tools/pico429.py`, `tools/arinc429_codec.py`, and `tools/README.md`. |
| **IR-10** | 🟡 **Major** | Profile Configuration | Global `opt-level = 3` in workspace `Cargo.toml` bloats firmware flash. | Use package-specific profile override: `[profile.release.package.pico429] opt-level = 3`. |
| **IR-11** | 🟢 **Nit** | Dependency Features | `rmcp::transport::stdio` requires the `transport-io` feature flag; `rmcp-macros` should not be pinned separately. | Specify `rmcp = { version = "3.0", features = ["server", "macros", "transport-io"] }`. |
| **IR-12** | 🟢 **Nit** | Library Reuse | Document specifies implementing 1-byte stream scanner in host transport, ignoring existing helper. | Directly reuse existing `arinc429_core::protocol::find_frame_in_stream`. |
| **IR-13** | 🟢 **Nit** | Variable Casing | `cumulative_consumed_counter` uses lowercase `snake_case` for a static array in `channel.rs`. | Rename to `pub static CUMULATIVE_CONSUMED: [AtomicU32; ACTIVE_CHANNEL_COUNT]`. |
| **IR-14** | 🟢 **Nit** | Sequence Bit-Width | Test description mentions 32-bit sequence number wrap-around, but wire frame `seq_num` is 16 bits. | Clarify 16-bit wire `seq_num` rollover vs 32-bit cumulative consumption counter arithmetic. |
| **IR-15** | 🟢 **Nit** | Document Formatting | Remove informal footer `*Drafted with EGM*`. | Drop footer footnote. |
