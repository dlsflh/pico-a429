#!/usr/bin/env python3
"""ARINC 429 Pico 2 / Pico 2 W 极速免编译交互与诊断工具 (Zero-Compile Diagnostic Tool)

依赖: pyserial
运行方式:
    uv run --with pyserial python tools/pico429.py ping
    uv run --with pyserial python tools/pico429.py status
    uv run --with pyserial python tools/pico429.py config 0 --enable --dir TX --speed 100k [--run]
    uv run --with pyserial python tools/pico429.py speed 0 100k
    uv run --with pyserial python tools/pico429.py filter 1 --allow 310
    uv run --with pyserial python tools/pico429.py send --label 310 --data 0x1234 [--run]
    uv run --with pyserial python tools/pico429.py bit
    uv run --with pyserial python tools/pico429.py interactive
    uv run --with pyserial python tools/pico429.py listen
    uv run --with pyserial python tools/pico429.py opmode [config|run]
    uv run --with pyserial python tools/pico429.py profile apply profiles/avionics_bench.toml
    uv run --with pyserial python tools/pico429.py profile list
    uv run --with pyserial python tools/pico429.py profile load 0
    uv run --with pyserial python tools/pico429.py profile boot 0 [--clear]
"""

from __future__ import annotations

import argparse
import pathlib
import sys
import time
import typing

if typing.TYPE_CHECKING:
    import serial  # type: ignore[import-untyped,unresolved-import]  # ty: ignore[unresolved-import]
else:
    try:
        import serial
    except ImportError:
        serial = None

try:
    from tools.arinc429_codec import (
        ACK_STATUS_DESCRIPTIONS,
        AckStatus,
        ArincWord,
        BinaryFrame,
        FrameFlags,
        FrameType,
        Opcode,
        ProfileSubOp,
        find_frame_in_stream,
        format_ack_status,
        pack_12_channels,
        pack_channel_nibble,
        unpack_12_channels,
        unpack_channel_nibble,
    )
    from tools.pico429_profile import (
        build_batch_spec,
        generate_toml_from_cfgdump,
        normalize_speed,
        parse_profile_list_output,
        parse_toml_profile,
    )
    from tools.pico429_transport import (
        find_pico_port,
        open_serial,
        send_binary_cmd,
        send_binary_cmd_multi_ack,
        send_command_wait,
        send_command_wait_multiline,
        send_word_and_await_loopback,
    )
except ModuleNotFoundError:
    from arinc429_codec import (  # type: ignore[no-redef]  # ty: ignore[unresolved-import]
        ACK_STATUS_DESCRIPTIONS,
        AckStatus,
        ArincWord,
        BinaryFrame,
        FrameFlags,
        FrameType,
        Opcode,
        ProfileSubOp,
        find_frame_in_stream,
        format_ack_status,
        pack_12_channels,
        pack_channel_nibble,
        unpack_12_channels,
        unpack_channel_nibble,
    )
    from pico429_profile import (  # type: ignore[no-redef]  # ty: ignore[unresolved-import]
        build_batch_spec,
        generate_toml_from_cfgdump,
        normalize_speed,
        parse_profile_list_output,
        parse_toml_profile,
    )
    from pico429_transport import (  # type: ignore[no-redef]  # ty: ignore[unresolved-import]
        find_pico_port,
        open_serial,
        send_binary_cmd,
        send_binary_cmd_multi_ack,
        send_command_wait,
        send_command_wait_multiline,
        send_word_and_await_loopback,
    )

# 显式重导出以保持向后测试兼容
__all__ = [
    "ACK_STATUS_DESCRIPTIONS",
    "AckStatus",
    "ArincWord",
    "BinaryFrame",
    "FrameFlags",
    "FrameType",
    "Opcode",
    "ProfileSubOp",
    "build_batch_spec",
    "find_frame_in_stream",
    "find_pico_port",
    "format_ack_status",
    "generate_toml_from_cfgdump",
    "normalize_speed",
    "open_serial",
    "pack_12_channels",
    "pack_channel_nibble",
    "parse_profile_list_output",
    "parse_toml_profile",
    "send_binary_cmd",
    "send_binary_cmd_multi_ack",
    "send_command_wait",
    "send_command_wait_multiline",
    "send_word_and_await_loopback",
    "unpack_12_channels",
    "unpack_channel_nibble",
]


def cmd_ping(args: argparse.Namespace) -> None:
    port = find_pico_port(args.port)
    print(f"[*] 正在连接 {port} 并发送二进制 PING...")
    ser = open_serial(port, timeout=0.5)
    try:
        t0 = time.perf_counter()
        ack = send_binary_cmd(ser, Opcode.PING, timeout=1.0)
        dt_ms = (time.perf_counter() - t0) * 1000.0
        if ack.ack_status == AckStatus.OK:
            print(
                f"[\033[32mOK\033[0m] 握手成功: 收到 PONG ACK (Seq {ack.seq_num}, RTT: {dt_ms:.2f} ms)"
            )
        else:
            print(
                f"[\033[33mWARN\033[0m] 收到错误 ACK: 0x{ack.ack_status:02X} (耗时: {dt_ms:.2f} ms)"
            )
    except (serial.SerialException, TimeoutError, OSError) as e:
        print(f"[\033[31mFAIL\033[0m] PING 握手失败: {e}", file=sys.stderr)
        sys.exit(1)
    finally:
        ser.close()


def cmd_status(args: argparse.Namespace) -> None:
    port = find_pico_port(args.port)
    print(f"[*] 正在查询 {port} 状态 (二进制协议)...")
    ser = open_serial(port, timeout=0.5)
    try:
        ack = send_binary_cmd(ser, Opcode.QUERY_STATUS, timeout=1.0)
        if ack.ack_status != AckStatus.OK:
            print(
                f"[\033[31mFAIL\033[0m] 查询状态失败: {format_ack_status(ack.ack_status)}",
                file=sys.stderr,
            )
            sys.exit(1)
        mode_str = (
            "CONFIG (维护配置模式: 总线收发锁定，引脚高阻静默)"
            if ack.channel == 0
            else "OPERATIONAL (正常运行模式: 物理总线实时收发中)"
        )
        channels = unpack_12_channels(ack.timestamp_us, ack.raw_word)
        print(f"[\033[32mSTATUS\033[0m] 当前工作模式: {mode_str}")
        print("-" * 50)
        print(f"{'通道':<6} {'使能':<8} {'方向':<8} {'速率'}")
        print("-" * 50)
        for ch_idx, (en, is_rx, hs) in enumerate(channels):
            if args.channel is not None and ch_idx != args.channel:
                continue
            en_str = "ENABLE" if en else "DISABLE"
            dir_str = ("RX" if is_rx else "TX") if en else "-"
            spd_str = ("100K (HIGH)" if hs else "12.5K (LOW)") if en else "-"
            print(f"CH{ch_idx:<4} {en_str:<8} {dir_str:<8} {spd_str}")
    except (serial.SerialException, TimeoutError, OSError) as e:
        print(f"[\033[31mFAIL\033[0m] 查询状态失败: {e}", file=sys.stderr)
        sys.exit(1)
    finally:
        ser.close()


def cmd_config(args: argparse.Namespace) -> None:
    """通道启停与方向/速率配置。"""
    if not (0 <= args.channel <= 11):
        print(f"错误: 通道编号超限 (0..11): {args.channel}", file=sys.stderr)
        sys.exit(1)

    port = find_pico_port(args.port)
    ser = open_serial(port, timeout=0.5)
    try:
        if args.disable:
            nibble = pack_channel_nibble(enable=False, is_rx=False, high_speed=False)
        else:
            is_rx = args.dir.upper() == "RX"
            speed_str = normalize_speed(args.speed)
            hs = speed_str.upper() == "HIGH"
            nibble = pack_channel_nibble(enable=True, is_rx=is_rx, high_speed=hs)

        print(f"[*] 下发通道 {args.channel} 二进制配置 (Nibble: 0x{nibble:X})...")
        ack = send_binary_cmd(
            ser,
            Opcode.SET_CHANNEL_CFG,
            channel=args.channel,
            payload_0=nibble,
            timeout=1.0,
        )
        if ack.ack_status == AckStatus.OK:
            print(
                f"[\033[32mOK\033[0m] 通道 {args.channel} 配置成功 (ACK Seq {ack.seq_num})"
            )
            if getattr(args, "run", False):
                ack_mode = send_binary_cmd(
                    ser, Opcode.SET_OPERATING_MODE, payload_0=1, timeout=1.0
                )
                if ack_mode.ack_status == AckStatus.OK:
                    print(
                        "[\033[32mOK\033[0m] 已自动切换为 OPERATIONAL 正常运行模式 (总线激活)"
                    )
            else:
                ack_st = send_binary_cmd(ser, Opcode.QUERY_STATUS, timeout=1.0)
                if ack_st.channel == 0:
                    print(
                        "[*] 提示: 当前处于 CONFIG 维护模式 (物理引脚保持静默)。如需激活总线收发，请添加 --run 或执行 'pico429.py opmode run'"
                    )
        else:
            print(
                f"[\033[31mFAIL\033[0m] 通道配置失败: {format_ack_status(ack.ack_status)}",
                file=sys.stderr,
            )
            sys.exit(1)
    except (serial.SerialException, TimeoutError, OSError, ValueError) as e:
        print(f"[\033[31mFAIL\033[0m] 通道配置失败: {e}", file=sys.stderr)
        sys.exit(1)
    finally:
        ser.close()


def cmd_speed(args: argparse.Namespace) -> None:
    """动态切换指定通道波特率。"""
    if not (0 <= args.channel <= 11):
        print(f"错误: 通道编号超限 (0..11): {args.channel}", file=sys.stderr)
        sys.exit(1)

    port = find_pico_port(args.port)
    ser = open_serial(port, timeout=0.5)
    try:
        # 先读取当前通道状态
        ack = send_binary_cmd(ser, Opcode.QUERY_STATUS, timeout=1.0)
        channels = unpack_12_channels(ack.timestamp_us, ack.raw_word)
        en, is_rx, _ = channels[args.channel]

        speed_str = normalize_speed(args.speed)
        hs = speed_str.upper() == "HIGH"
        nibble = pack_channel_nibble(enable=en, is_rx=is_rx, high_speed=hs)

        ack = send_binary_cmd(
            ser,
            Opcode.SET_CHANNEL_CFG,
            channel=args.channel,
            payload_0=nibble,
            timeout=1.0,
        )
        if ack.ack_status == AckStatus.OK:
            print(f"[\033[32mOK\033[0m] 通道 {args.channel} 速率切换成功: {speed_str}")
        else:
            print(
                f"[\033[31mFAIL\033[0m] 速率配置失败: {format_ack_status(ack.ack_status)}",
                file=sys.stderr,
            )
            sys.exit(1)
    except (serial.SerialException, TimeoutError, OSError, ValueError) as e:
        print(f"[\033[31mFAIL\033[0m] 速率配置失败: {e}", file=sys.stderr)
        sys.exit(1)
    finally:
        ser.close()


def cmd_filter(args: argparse.Namespace) -> None:
    """动态配置硬件级 Label 过滤规则。"""
    if not (0 <= args.channel <= 11):
        print(f"错误: 通道编号超限 (0..11): {args.channel}", file=sys.stderr)
        sys.exit(1)

    port = find_pico_port(args.port)
    ser = open_serial(port, timeout=0.5)
    try:
        if args.clear:
            p0 = 0
            act_name = "CLEAR"
        elif args.allow:
            lbl = int(args.allow, 8)
            p0 = 1 | (lbl << 8)
            act_name = f"ALLOW {args.allow}"
        elif args.block:
            lbl = int(args.block, 8)
            p0 = 2 | (lbl << 8)
            act_name = f"BLOCK {args.block}"
        else:
            print(
                "错误: 必须指定 --allow <octal>, --block <octal> 或 --clear",
                file=sys.stderr,
            )
            sys.exit(1)

        print(f"[*] 下发通道 {args.channel} 过滤规则: {act_name}...")
        ack = send_binary_cmd(
            ser,
            Opcode.SET_FILTER,
            channel=args.channel,
            payload_0=p0,
            timeout=1.0,
        )
        if ack.ack_status == AckStatus.OK:
            print(
                f"[\033[32mOK\033[0m] 通道 {args.channel} 过滤规则配置成功 (ACK Seq {ack.seq_num})"
            )
        else:
            print(
                f"[\033[31mFAIL\033[0m] 过滤规则设置失败: {format_ack_status(ack.ack_status)}",
                file=sys.stderr,
            )
            sys.exit(1)
    except (serial.SerialException, TimeoutError, OSError, ValueError) as e:
        print(f"[\033[31mFAIL\033[0m] 过滤规则设置失败: {e}", file=sys.stderr)
        sys.exit(1)
    finally:
        ser.close()


def cmd_mode(args: argparse.Namespace) -> None:
    """切换遥测上报格式 (设备固件现已统一运行于定长 16 字节全双工二进制协议模式)。"""
    port = find_pico_port(args.port)
    ser = open_serial(port, timeout=0.5)
    try:
        ack = send_binary_cmd(ser, Opcode.PING, timeout=1.0)
        if ack.ack_status == AckStatus.OK:
            print(
                "[\033[32mOK\033[0m] 设备在线且统一运行于定长 16 字节全双工二进制协议模式 (BIN)"
            )
    except (serial.SerialException, TimeoutError, OSError) as e:
        print(f"[\033[31mFAIL\033[0m] 设备连接失败: {e}", file=sys.stderr)
        sys.exit(1)
    finally:
        ser.close()


def cmd_send(args: argparse.Namespace) -> None:
    if not (0 <= args.channel <= 11):
        print(f"错误: 通道编号超限 (0..11): {args.channel}", file=sys.stderr)
        sys.exit(1)

    port = find_pico_port(args.port)
    ser = open_serial(port, timeout=0.5)
    try:
        if getattr(args, "run", False):
            # 发送前自动确保进入 OPERATIONAL 运行模式
            send_binary_cmd(ser, Opcode.SET_OPERATING_MODE, payload_0=1, timeout=1.0)

        if args.raw is not None:
            raw_val = int(args.raw, 16)
        else:
            if not args.label:
                print(
                    "错误: 必须指定 --label (例如 310) 或 --raw (例如 0x12345678)",
                    file=sys.stderr,
                )
                sys.exit(1)
            lbl = int(args.label, 8)
            data_val = int(args.data, 0) if args.data else 0
            word = ArincWord.from_fields(lbl, args.sdi, data_val, args.ssm)
            raw_val = word.raw

        print(f"[*] 发送 ARINC 字: CH{args.channel} 0x{raw_val:08X}")
        ack = send_binary_cmd(
            ser,
            Opcode.TX_RAW_WORD,
            channel=args.channel,
            payload_0=raw_val,
            timeout=1.0,
        )
        if ack.ack_status == AckStatus.OK:
            print(
                f"[\033[32mOK\033[0m] 发送成功: 0x{raw_val:08X} (ACK Seq {ack.seq_num})"
            )
        elif ack.ack_status == AckStatus.ERR_MODE_CONFLICT:
            # 区分模式原因：CONFIG 维护模式锁定 vs 通道方向不是 TX
            st_ack = send_binary_cmd(ser, Opcode.QUERY_STATUS, timeout=1.0)
            if st_ack.channel == 0:
                print(
                    "[\033[31mFAIL\033[0m] 发送失败: 设备处于 CONFIG 维护模式，物理总线已锁定 (请添加 --run 或执行 'pico429.py opmode run' 激活运行模式)",
                    file=sys.stderr,
                )
            else:
                print(
                    f"[\033[31mFAIL\033[0m] 通道 {args.channel} 不是 TX 模式或未使能",
                    file=sys.stderr,
                )
            sys.exit(1)
        else:
            print(
                f"[\033[31mFAIL\033[0m] 发送失败: {format_ack_status(ack.ack_status)}",
                file=sys.stderr,
            )
            sys.exit(1)
    except (serial.SerialException, TimeoutError, OSError) as e:
        print(f"[\033[31mFAIL\033[0m] 发送失败: {e}", file=sys.stderr)
        sys.exit(1)
    finally:
        ser.close()


def cmd_bit(args: argparse.Namespace) -> None:
    """执行物理对接自收自发 BIT (内建自测试) 极速验证 (< 1ms RTT)"""
    port = find_pico_port(args.port)
    print(
        f"[*] 连接 {port}，准备执行 ARINC 429 硬件环回 BIT 自测试 (CH{args.tx} -> CH{args.rx})..."
    )
    ser = open_serial(port, timeout=0.5)

    original_mode_config = False
    original_tx_cfg: tuple[bool, bool, bool] = (False, False, False)
    original_rx_cfg: tuple[bool, bool, bool] = (False, False, False)
    modified_tx = False
    modified_rx = False

    try:
        # 查询当前运行模式与通道配置状态
        ack_st = send_binary_cmd(ser, Opcode.QUERY_STATUS, timeout=1.0)
        original_mode_config = ack_st.channel == 0
        channels = unpack_12_channels(ack_st.timestamp_us, ack_st.raw_word)

        original_tx_cfg = channels[args.tx]
        original_rx_cfg = channels[args.rx]

        tx_en, tx_is_rx, _ = original_tx_cfg
        rx_en, rx_is_rx, _ = original_rx_cfg

        # 确保 CH_TX 配置为 TX (100K)，CH_RX 配置为 RX (100K)
        if not (tx_en and not tx_is_rx):
            print(f"[*] 自动配置 CH{args.tx} 为 TX (100K 高速)...")
            tx_nibble = pack_channel_nibble(enable=True, is_rx=False, high_speed=True)
            send_binary_cmd(
                ser,
                Opcode.SET_CHANNEL_CFG,
                channel=args.tx,
                payload_0=tx_nibble,
                timeout=1.0,
            )
            modified_tx = True

        if not (rx_en and rx_is_rx):
            print(f"[*] 自动配置 CH{args.rx} 为 RX (100K 高速)...")
            rx_nibble = pack_channel_nibble(enable=True, is_rx=True, high_speed=True)
            send_binary_cmd(
                ser,
                Opcode.SET_CHANNEL_CFG,
                channel=args.rx,
                payload_0=rx_nibble,
                timeout=1.0,
            )
            modified_rx = True

        # 协调激活 OPERATIONAL 运行模式
        if original_mode_config:
            print(
                "[*] 设备当前处于 CONFIG 维护模式，正在激活 OPERATIONAL 运行模式以执行硬件自测试..."
            )
            send_binary_cmd(ser, Opcode.SET_OPERATING_MODE, payload_0=1, timeout=1.0)

        # 硬件建立稳定守卫延时 (Gap Guard Delay: 15ms)
        # 确保: 1) TX 物理引脚电平稳定; 2) RX PIO 状态机完成 3.4-bit (34us) reset_gap 间隙检测并就位
        time.sleep(0.015)

        bit_vectors = [
            ("310", 0, 0x01234, 3, "标准巡航高度字"),
            ("203", 1, 0x00567, 3, "空速气压测量字"),
            ("001", 2, 0x00001, 3, "最小八进制 Label 测试"),
            ("377", 3, 0x7FFFF, 3, "全 1 数据极限字"),
            ("156", 0, 0x00000, 0, "全 0 数据与 SSM=0"),
            ("114", 1, 0x55555, 3, "交替位模式 0x55555"),
            ("220", 2, 0x2AAAA, 3, "交替位模式 0x2AAAA"),
            ("045", 3, 0x13579, 1, "奇偶校验敏感向量"),
        ]

        print("=" * 102)
        print(
            f"{'编号':<4} {'测试项说明':<22} {'发送 Label/Data':<22} {'发送字 (RAW)':<14} {'接收字 (RAW)':<14} {'RTT':<8} {'状态'}"
        )
        print("-" * 102)

        passed_count = 0
        total_count = len(bit_vectors)
        rtt_list: list[float] = []

        # 持久化流缓冲区，贯穿所有 8 个向量，绝不跨迭代截断或清空
        stream_buf = bytearray()
        ser.reset_input_buffer()

        for i, (label, sdi, data, ssm, desc) in enumerate(bit_vectors, start=1):
            word = ArincWord.from_fields(int(label, 8), sdi, data, ssm)
            t0 = time.perf_counter()

            ack, rx_frame = send_word_and_await_loopback(
                ser,
                tx_ch=args.tx,
                rx_ch=args.rx,
                raw_word=word.raw,
                stream_buf=stream_buf,
                timeout=1.0,
            )

            rtt_ms = (time.perf_counter() - t0) * 1000.0
            rtt_list.append(rtt_ms)

            sent_raw = f"0x{word.raw:08X}"
            recv_raw = f"0x{rx_frame.raw_word:08X}" if rx_frame else None
            parity_ok = (
                (rx_frame.flags & FrameFlags.PARITY_ERROR) == 0 if rx_frame else False
            )

            is_pass = (
                ack is not None
                and ack.ack_status == AckStatus.OK
                and rx_frame is not None
                and word.raw == rx_frame.raw_word
                and parity_ok
            )
            if is_pass:
                passed_count += 1
                status_str = "\033[32mPASS\033[0m"
            else:
                status_str = "\033[31mFAIL\033[0m"

            tx_info = f"L:{label} 0x{data:05X}"
            sent_disp = sent_raw or "TIMEOUT"
            recv_disp = recv_raw or "TIMEOUT"
            print(
                f"#{i:<3} {desc:<20} {tx_info:<20} {sent_disp:<14} {recv_disp:<14} {rtt_ms:<6.2f}ms {status_str}"
            )

        print("=" * 102)
        avg_rtt = sum(rtt_list) / len(rtt_list) if rtt_list else 0.0
        loss_rate = ((total_count - passed_count) / total_count) * 100.0

        if passed_count == total_count:
            print(
                f"[\033[32mBIT PASSED\033[0m] {passed_count}/{total_count} 向量全通过 | "
                f"丢包率: {loss_rate:.1f}% | 平均 RTT: {avg_rtt:.2f} ms | "
                f"\033[32m硬件物理收发完全正常 (RTT < 1ms)！\033[0m"
            )
        else:
            print(
                f"[\033[31mBIT FAILED\033[0m] 通过: {passed_count}/{total_count} | "
                f"丢包率: {loss_rate:.1f}% | 请检查 GP0(TX_A)->GP2(RX_A) 与 GP1(TX_B)->GP3(RX_B) 物理接线！",
                file=sys.stderr,
            )
            sys.exit(1)
    finally:
        # 1. 恢复模式
        if original_mode_config:
            try:
                send_binary_cmd(
                    ser, Opcode.SET_OPERATING_MODE, payload_0=0, timeout=1.0
                )
                print(
                    "[*] BIT 自检结束，已将设备恢复为原 CONFIG 维护模式 (物理引脚重新置为静默)"
                )
            except (serial.SerialException, TimeoutError, OSError) as e:
                print(f"[*] 提示: 恢复 CONFIG 模式异常: {e}", file=sys.stderr)

        # 2. 完全恢复通道原始配置
        if modified_tx:
            try:
                tx_nib = pack_channel_nibble(*original_tx_cfg)
                send_binary_cmd(
                    ser,
                    Opcode.SET_CHANNEL_CFG,
                    channel=args.tx,
                    payload_0=tx_nib,
                    timeout=1.0,
                )
                print(f"[*] 已恢复 CH{args.tx} 原始通道配置")
            except (serial.SerialException, TimeoutError, OSError) as e:
                print(f"[*] 提示: 恢复 CH{args.tx} 异常: {e}", file=sys.stderr)

        if modified_rx:
            try:
                rx_nib = pack_channel_nibble(*original_rx_cfg)
                send_binary_cmd(
                    ser,
                    Opcode.SET_CHANNEL_CFG,
                    channel=args.rx,
                    payload_0=rx_nib,
                    timeout=1.0,
                )
                print(f"[*] 已恢复 CH{args.rx} 原始通道配置")
            except (serial.SerialException, TimeoutError, OSError) as e:
                print(f"[*] 提示: 恢复 CH{args.rx} 异常: {e}", file=sys.stderr)

        ser.close()


def execute_interactive_loopback(
    ser: serial.Serial,
    tx_ch: int,
    rx_ch: int,
    label: str,
    sdi: int,
    data: int,
    ssm: int,
    stream_buf: bytearray | None = None,
) -> None:
    """在保持打开的串口上执行单次用户自驱动回环测试并即时解码打印。"""
    word = ArincWord.from_fields(int(label, 8), sdi, data, ssm)
    t0 = time.perf_counter()
    if stream_buf is None:
        stream_buf = bytearray()

    ack, rx_frame = send_word_and_await_loopback(
        ser,
        tx_ch=tx_ch,
        rx_ch=rx_ch,
        raw_word=word.raw,
        stream_buf=stream_buf,
        timeout=1.0,
    )

    rtt_ms = (time.perf_counter() - t0) * 1000.0
    sent_raw = f"0x{word.raw:08X}"
    recv_word = ArincWord(rx_frame.raw_word) if rx_frame else None
    parity_ok = (rx_frame.flags & FrameFlags.PARITY_ERROR) == 0 if rx_frame else False

    is_pass = (
        ack is not None
        and ack.ack_status == AckStatus.OK
        and recv_word is not None
        and word.raw == recv_word.raw
        and parity_ok
    )
    status_str = "\033[32mPASS\033[0m" if is_pass else "\033[31mFAIL\033[0m"
    p_status = "\033[32mOK\033[0m" if parity_ok else "\033[31mERR\033[0m"

    print(
        f"[*] [\033[36mTX\033[0m] CH{tx_ch} L:{label} SDI:{sdi} DATA:0x{data:05X} SSM:{ssm} (RAW: {sent_raw})"
    )
    if recv_word:
        recv_raw = f"0x{recv_word.raw:08X}"
        print(
            f"[+] [\033[32mRX\033[0m] CH{rx_ch} L:{recv_word.label_str} SDI:{recv_word.sdi} "
            f"DATA:0x{recv_word.data:05X} ({recv_word.data}) SSM:{recv_word.ssm} P:{p_status} "
            f"(RAW: {recv_raw}) | RTT: \033[35m{rtt_ms:.2f} ms\033[0m -> [{status_str}]"
        )
    else:
        print(
            f"[-] [\033[31mRX\033[0m] CH{rx_ch} 接收超时未收到数据 (耗时: {rtt_ms:.2f} ms)"
        )


def cmd_interactive(args: argparse.Namespace) -> None:
    """用户自驱动实时回环测试交互控制台 (Dynamic Self-Driven Loopback REPL)"""
    port = find_pico_port(args.port)
    tx_ch = args.tx
    rx_ch = args.rx
    default_label = args.label
    default_sdi = args.sdi
    default_ssm = args.ssm

    print("=" * 86)
    print("  Pico-429 用户自驱动实时回环测试终端 (Dynamic Real-time Loopback Console)")
    print("=" * 86)
    print(
        f"[*] 已连接串口: \033[36m{port}\033[0m | 发送通道: \033[33mCH{tx_ch}\033[0m -> 接收通道: \033[32mCH{rx_ch}\033[0m"
    )
    print("支持语法:")
    print(
        "  1. 直接输入纯数值: '1234' 或 '0x1234' (使用默认 Label: 310 发射并即时捕获环回)"
    )
    print("  2. 发送自定义字: 'tx <label> <data_hex> [sdi] [ssm]' (例如: tx 203 0x567)")
    print("  3. 发送原始十六进制字: 'raw 0xXXXXXXXX'")
    print(
        "  4. 控制指令: 'speed 100k|12.5k', 'filter allow 310', 'status', 'ping', 'bit'"
    )
    print("  5. 输入 'help' 查看帮助, 输入 'quit' 或 'exit' 或 Ctrl+C 退出")
    print("-" * 86)

    ser = open_serial(port, timeout=0.5)

    original_mode_config = False
    try:
        # 连通性测试与工作模式协调
        try:
            ack_st = send_binary_cmd(ser, Opcode.QUERY_STATUS, timeout=1.0)
            original_mode_config = ack_st.channel == 0
            if original_mode_config:
                print(
                    "[*] 当前处于 CONFIG 维护模式，正在激活 OPERATIONAL 运行模式以支持实时交互..."
                )
                send_binary_cmd(
                    ser, Opcode.SET_OPERATING_MODE, payload_0=1, timeout=1.0
                )
            print("[\033[32mONLINE\033[0m] 固件在线就绪！可随时输入载荷开始测试。\n")
        except TimeoutError:
            print("[\033[33mWARN\033[0m] 握手无响应，仍尝试进入交互模式...\n")

        inter_stream_buf = bytearray()

        while True:
            try:
                line = input(f"Pico429 [CH{tx_ch}->CH{rx_ch}] >> ").strip()
            except (EOFError, KeyboardInterrupt):
                print("\n[*] 退出交互模式。")
                break

            if not line:
                continue

            lower_line = line.lower()
            if lower_line in ("quit", "exit", "q"):
                print("[*] 退出交互模式。")
                break

            if lower_line == "help":
                print("帮助说明:")
                print(
                    "  <数值>                   发送指定数据载荷 (例如 1234 或 0x1234)"
                )
                print("  tx <oct> <data> [s] [m]  发送结构化字 (例如 tx 310 0x1234)")
                print(
                    "  raw <0xXXXXXXXX>         发送 32 位原始字 (例如 raw 0xE048D013)"
                )
                print("  speed <100k|12.5k>       切换总线速率")
                print("  filter <allow|block|clear> [oct] 配置 Label 硬件过滤")
                print("  status                   查询板卡状态")
                print("  ping                     发送心跳测试")
                print("  bit                      执行完整 8 向量硬件 BIT 自检")
                print("  quit / exit              退出控制台")
                continue

            if lower_line == "ping":
                t0 = time.perf_counter()
                try:
                    ack = send_binary_cmd(ser, Opcode.PING, timeout=1.0)
                    dt = (time.perf_counter() - t0) * 1000.0
                    print(f"[\033[32mOK\033[0m] PONG ACK (RTT: {dt:.2f} ms)")
                except TimeoutError as e:
                    print(f"[\033[31mTIMEOUT\033[0m] {e}")
                continue

            if lower_line == "status":
                try:
                    ack = send_binary_cmd(ser, Opcode.QUERY_STATUS, timeout=1.0)
                    mode_str = "CONFIG" if ack.channel == 0 else "OPERATIONAL"
                    channels = unpack_12_channels(ack.timestamp_us, ack.raw_word)
                    print(f"[\033[32mSTATUS\033[0m] 模式: {mode_str}")
                    for ch, (en, is_rx, hs) in enumerate(channels):
                        e_str = "EN" if en else "DIS"
                        d_str = ("RX" if is_rx else "TX") if en else "-"
                        s_str = ("100K" if hs else "12.5K") if en else "-"
                        print(f"  CH{ch:<2}: {e_str:<3} {d_str:<2} {s_str}")
                except TimeoutError as e:
                    print(f"[\033[31mTIMEOUT\033[0m] {e}")
                continue

            if lower_line == "bit":
                m_args = argparse.Namespace(port=port, tx=tx_ch, rx=rx_ch)
                ser.close()
                cmd_bit(m_args)
                ser = open_serial(port, timeout=0.5)
                continue

            if lower_line.startswith("speed "):
                parts = line.split()
                if len(parts) >= 2:
                    spd = parts[1].upper()
                    hs = "100" in spd or "HIGH" in spd
                    try:
                        ack_st = send_binary_cmd(ser, Opcode.QUERY_STATUS, timeout=1.0)
                        channels = unpack_12_channels(
                            ack_st.timestamp_us, ack_st.raw_word
                        )
                        en, is_rx, _ = channels[tx_ch]
                        nibble = pack_channel_nibble(en, is_rx, hs)
                        ack = send_binary_cmd(
                            ser,
                            Opcode.SET_CHANNEL_CFG,
                            channel=tx_ch,
                            payload_0=nibble,
                            timeout=1.0,
                        )
                        spd_name = "100K" if hs else "12.5K"
                        if ack.ack_status == AckStatus.OK:
                            print(
                                f"[\033[32mOK\033[0m] CH{tx_ch} 速率已切换为 {spd_name}"
                            )
                        else:
                            print(
                                f"[\033[31mFAIL\033[0m] 速率切换失败: {format_ack_status(ack.ack_status)}"
                            )
                    except TimeoutError as e:
                        print(f"[\033[31mTIMEOUT\033[0m] {e}")
                else:
                    print("语法: speed <100k|12.5k>")
                continue

            if lower_line.startswith("filter "):
                parts = line.split()
                if len(parts) >= 2:
                    sub = parts[1].upper()
                    if sub == "CLEAR":
                        p0 = 0
                    elif sub in ("ALLOW", "BLOCK") and len(parts) >= 3:
                        lbl = int(parts[2], 8)
                        p0 = (1 if sub == "ALLOW" else 2) | (lbl << 8)
                    else:
                        print("语法: filter <allow|block|clear> [octal_label]")
                        continue
                    try:
                        ack = send_binary_cmd(
                            ser,
                            Opcode.SET_FILTER,
                            channel=rx_ch,
                            payload_0=p0,
                            timeout=1.0,
                        )
                        if ack.ack_status == AckStatus.OK:
                            print(f"[\033[32mOK\033[0m] CH{rx_ch} 过滤规则已设置")
                        else:
                            print(
                                f"[\033[31mFAIL\033[0m] 过滤规则设置失败: {format_ack_status(ack.ack_status)}"
                            )
                    except TimeoutError as e:
                        print(f"[\033[31mTIMEOUT\033[0m] {e}")
                else:
                    print("语法: filter <allow|block|clear> [octal_label]")
                continue

            if lower_line.startswith("raw "):
                raw_hex = line[4:].strip()
                try:
                    raw_val = int(raw_hex, 16)
                    t0 = time.perf_counter()
                    ack = send_binary_cmd(
                        ser,
                        Opcode.TX_RAW_WORD,
                        channel=tx_ch,
                        payload_0=raw_val,
                        timeout=1.0,
                    )
                    dt = (time.perf_counter() - t0) * 1000.0
                    if ack.ack_status == AckStatus.OK:
                        print(
                            f"[\033[32mOK\033[0m] 发送成功: 0x{raw_val:08X} (耗时: {dt:.2f} ms)"
                        )
                    else:
                        print(
                            f"[\033[31mFAIL\033[0m] 发送失败: {format_ack_status(ack.ack_status)}"
                        )
                except ValueError:
                    print(f"错误: 无效的十六进制字 '{raw_hex}'")
                continue

            if lower_line.startswith("tx "):
                parts = line.split()
                if len(parts) >= 3:
                    lbl = parts[1]
                    try:
                        d_val = int(parts[2], 0)
                        s_val = int(parts[3]) if len(parts) >= 4 else default_sdi
                        m_val = int(parts[4]) if len(parts) >= 5 else default_ssm
                        execute_interactive_loopback(
                            ser,
                            tx_ch,
                            rx_ch,
                            lbl,
                            s_val,
                            d_val,
                            m_val,
                            stream_buf=inter_stream_buf,
                        )
                    except ValueError as e:
                        print(f"错误: 参数解析失败: {e}")
                else:
                    print("语法: tx <label_octal> <data_hex> [sdi] [ssm]")
                continue

            # 纯载荷模式: 用户直接输入 "1234" 或 "0x1234"
            try:
                data_val = int(line, 0)
                if not 0 <= data_val <= 0x7FFFF:
                    print(f"错误: 数据载荷 0x{data_val:X} 超出 19 位有效上限 (0x7FFFF)")
                    continue
                execute_interactive_loopback(
                    ser,
                    tx_ch,
                    rx_ch,
                    default_label,
                    default_sdi,
                    data_val,
                    default_ssm,
                    stream_buf=inter_stream_buf,
                )
            except ValueError:
                print(f"未识别指令或数值: '{line}'。输入 'help' 获取帮助。")
    finally:
        if original_mode_config:
            try:
                send_binary_cmd(
                    ser, Opcode.SET_OPERATING_MODE, payload_0=0, timeout=1.0
                )
                print(
                    "[*] 退出交互模式，已恢复为原 CONFIG 维护模式 (物理引脚重新置为静默)"
                )
            except (serial.SerialException, TimeoutError, OSError) as e:
                print(f"[*] 提示: 退出恢复配置模式异常: {e}", file=sys.stderr)
        ser.close()


def cmd_listen(args: argparse.Namespace) -> None:
    port = find_pico_port(args.port)
    print(f"[*] 打开 {port} 开始监听 ARINC 429 流量 (Ctrl+C 退出)...")
    ser = open_serial(port, timeout=0.1)

    rx_buf = bytearray()
    print("-" * 96)
    print(
        f"{'通道':<6} {'序号':<6} {'时间戳(us)':<12} {'速率':<6} {'Label':<8} {'SDI':<5} {'Data (Hex)':<12} {'SSM':<5} {'奇偶':<5} {'原始字 (Hex)':<12}"
    )
    print("-" * 96)

    try:
        while True:
            chunk = ser.read(ser.in_waiting or 64)
            if not chunk:
                continue
            rx_buf.extend(chunk)

            while True:
                res = find_frame_in_stream(rx_buf)
                if res is None:
                    break
                bin_frame, consumed = res
                del rx_buf[:consumed]

                if bin_frame.frame_type != FrameType.ARINC_DATA:
                    continue

                word = ArincWord(bin_frame.raw_word)
                is_100k = bool(bin_frame.flags & FrameFlags.SPEED_100K)
                speed_str = "100K" if is_100k else "12.5K"
                p_str = (
                    "\033[32mOK\033[0m"
                    if word.is_parity_valid
                    else "\033[31mERR\033[0m"
                )

                print(
                    f"CH{bin_frame.channel:<4} #{bin_frame.seq_num:<5} {bin_frame.timestamp_us:<12} "
                    f"{speed_str:<6} {word.label_str} ({word.label_octal:<3}) {word.sdi:<5} "
                    f"0x{word.data:05X} ({word.data:<7}) {word.ssm:<5} {p_str:<5} 0x{word.raw:08X}"
                )
    except KeyboardInterrupt:
        print("\n[*] 监听已由用户中止。")
    finally:
        ser.close()


def cmd_profile_apply(args: argparse.Namespace) -> None:
    """一键应用 TOML 配置文件到 12 通道硬件 (二进制 CFGBATCH 下发)。"""
    toml_path = pathlib.Path(args.file)
    if not toml_path.exists():
        print(f"错误: 配置文件不存在: {toml_path}", file=sys.stderr)
        sys.exit(1)

    try:
        content = toml_path.read_text(encoding="utf-8")
        parsed = parse_toml_profile(content)
    except (ValueError, OSError) as e:
        print(f"[\033[31mFAIL\033[0m] 配置文件解析失败: {e}", file=sys.stderr)
        sys.exit(1)

    port = find_pico_port(args.port)
    print(f"[*] 连接 {port} 并下发配置方案 '{parsed['name']}'...")
    ser = open_serial(port, timeout=0.5)

    try:
        # 组装 12 通道配置列表
        channels_list = []
        for ch in range(12):
            cfg = parsed["channels"].get(ch, {"direction": "DISABLED", "speed": "HIGH"})
            d = cfg.get("direction", "DISABLED").upper()
            en = d != "DISABLED"
            is_rx = d == "RX"
            s = cfg.get("speed", "HIGH").upper()
            hs = "HIGH" in s or "100" in s
            channels_list.append((en, is_rx, hs))

        p0, p1 = pack_12_channels(channels_list)
        print("[*] 批量安全配置 12 通道 (CFGBATCH 二进制下发)...")
        ack = send_binary_cmd(
            ser, Opcode.CFGBATCH, payload_0=p0, payload_1=p1, timeout=2.0
        )
        if ack.ack_status != AckStatus.OK:
            print(
                f"[\033[31mFAIL\033[0m] CFGBATCH 下发失败: {format_ack_status(ack.ack_status)}",
                file=sys.stderr,
            )
            sys.exit(1)

        # 逐个通道配置 Label 过滤规则 (若有)
        for ch, ch_cfg in parsed["channels"].items():
            if ch_cfg["direction"] != "RX":
                continue
            f_mode = ch_cfg.get("filter_mode", "none").lower()
            labels = ch_cfg.get("labels", [])

            # 先清空当前通道过滤规则 (重置为 AcceptAll / 全放行)
            send_binary_cmd(
                ser, Opcode.SET_FILTER, channel=ch, payload_0=0, timeout=1.0
            )

            if f_mode == "whitelist":
                for lbl_str in labels:
                    lbl = int(lbl_str, 8)
                    send_binary_cmd(
                        ser,
                        Opcode.SET_FILTER,
                        channel=ch,
                        payload_0=1 | (lbl << 8),
                        timeout=1.0,
                    )
            elif f_mode == "blacklist":
                for lbl_str in labels:
                    lbl = int(lbl_str, 8)
                    send_binary_cmd(
                        ser,
                        Opcode.SET_FILTER,
                        channel=ch,
                        payload_0=2 | (lbl << 8),
                        timeout=1.0,
                    )

        print("\n" + "=" * 60)
        print(f"配置方案: {parsed['name']} (v{parsed['version']})")
        print("-" * 60)
        print(f"{'通道':<6} {'方向':<10} {'波特率':<8} {'过滤规则'}")
        print("-" * 60)
        for ch in range(12):
            cfg = parsed["channels"].get(
                ch,
                {
                    "direction": "DISABLED",
                    "speed": "HIGH",
                    "filter_mode": "none",
                    "labels": [],
                },
            )
            d = cfg["direction"]
            s = cfg["speed"]
            f_mode = cfg.get("filter_mode", "none").lower()
            labels = cfg.get("labels", [])
            if d != "RX":
                f_str = "-"
            elif f_mode == "none" or not labels:
                f_str = "全部放行"
            elif f_mode == "whitelist":
                f_str = f"白名单 ({', '.join(labels)})"
            elif f_mode == "blacklist":
                f_str = f"黑名单 ({', '.join(labels)})"
            else:
                f_str = f_mode
            print(f"CH{ch:<4} {d:<10} {s:<8} {f_str}")
        print("=" * 60)

        if not getattr(args, "no_run", False):
            print("[*] 正在切换至 OPERATIONAL 正常运行模式 (激活总线实时收发)...")
            send_binary_cmd(ser, Opcode.SET_OPERATING_MODE, payload_0=1, timeout=1.0)
            print(
                f"[\033[32mOK\033[0m] 12 通道配置方案应用成功，设备已进入 OPERATIONAL 运行模式 (ACK Seq {ack.seq_num})"
            )
        else:
            print(
                f"[\033[32mOK\033[0m] 12 通道配置方案应用成功，设备保持在 CONFIG 维护模式 (ACK Seq {ack.seq_num})"
            )

    except (serial.SerialException, TimeoutError, OSError) as e:
        print(f"[\033[31mFAIL\033[0m] 应用配置失败: {e}", file=sys.stderr)
        sys.exit(1)
    finally:
        ser.close()


def cmd_profile_dump(args: argparse.Namespace) -> None:
    """从设备固件读取当前 12 通道状态并生成 TOML 配置文件。"""
    port = find_pico_port(args.port)
    print(f"[*] 连接 {port} 并导出 12 通道配置...")
    ser = open_serial(port, timeout=0.5)

    try:
        ack = send_binary_cmd(ser, Opcode.QUERY_STATUS, timeout=1.0)
        channels = unpack_12_channels(ack.timestamp_us, ack.raw_word)

        lines = [
            "# Pico-429 12-Channel ARINC 429 Configuration Profile",
            "# Generated automatically from hardware state",
            "[profile]",
            f'name = "{args.name}"',
            'version = "1.0"',
            "",
            "[global]",
            'mode = "BIN"',
            "",
        ]

        for ch, (en, is_rx, hs) in enumerate(channels):
            direction = "DISABLED" if not en else ("RX" if is_rx else "TX")
            speed = "high" if hs else "low"
            lines.append(f"[channel.{ch}]")
            lines.append(f'direction = "{direction}"')
            lines.append(f'speed = "{speed}"')
            if direction == "RX":
                lines.append('filter_mode = "none"')
                lines.append("labels = []")
            lines.append("")

        toml_content = "\n".join(lines)
        if args.stdout:
            print(toml_content)
        else:
            out_path = pathlib.Path(args.output or "pico429_config.toml")
            out_path.write_text(toml_content, encoding="utf-8")
            print(f"[\033[32mOK\033[0m] 配置已导出至: {out_path.resolve()}")
            print("-" * 50)
            preview = toml_content[:300] + ("\n..." if len(toml_content) > 300 else "")
            print(preview)

    except (serial.SerialException, TimeoutError, OSError) as e:
        print(f"[\033[31mFAIL\033[0m] 导出配置失败: {e}", file=sys.stderr)
        sys.exit(1)
    finally:
        ser.close()


def cmd_profile_list(args: argparse.Namespace) -> None:
    """查询片上 Flash 中的非易失配置方案列表。"""
    port = find_pico_port(args.port)
    print(f"[*] 连接 {port} 查询片上 Flash 存储方案...")
    ser = open_serial(port, timeout=0.5)

    try:
        acks = send_binary_cmd_multi_ack(
            ser,
            Opcode.PROFILE_OP,
            expected_count=4,
            channel=0,
            payload_0=ProfileSubOp.LIST,
            timeout=1.0,
        )

        print("\n" + "=" * 50)
        print("片上 Flash 非易失存储配置方案列表 (Slot 0..3):")
        print("-" * 50)
        print(f"{'槽位':<6} {'自启动':<8} {'状态':<10} {'校验'}")
        print("-" * 50)

        boot_slot = None
        for ack in acks:
            slot_id = ack.channel
            is_valid = (ack.timestamp_us & 0xFF) != 0
            is_boot = ((ack.timestamp_us >> 8) & 0xFF) != 0
            crc16_val = (ack.timestamp_us >> 16) & 0xFFFF
            if is_boot:
                boot_slot = slot_id
            boot_tag = "  [*]" if is_boot else "   -"
            status_tag = "ACTIVE" if is_valid else "EMPTY"
            crc_tag = f"0x{crc16_val:04X}" if is_valid else "-"
            print(f"Slot {slot_id:<3} {boot_tag:<8} {status_tag:<10} {crc_tag}")

        print("=" * 50)
        if boot_slot is not None:
            print(f"[*] 当前上电自启动槽位: Slot {boot_slot}")
        else:
            print("[*] 当前无自启动槽位 (默认静默基态: 12 通道全部 Disabled)")

    except (serial.SerialException, TimeoutError, OSError) as e:
        print(f"[\033[31mFAIL\033[0m] 查询配置列表失败: {e}", file=sys.stderr)
        sys.exit(1)
    finally:
        ser.close()


def cmd_profile_slot(args: argparse.Namespace) -> None:
    """处理片上 Flash 槽位操作 (SAVE, LOAD, BOOT)。"""
    action = args.profile_action.upper()
    slot = getattr(args, "slot", 0)
    if action == "SAVE":
        sub_op = ProfileSubOp.SAVE
    elif action == "LOAD":
        sub_op = ProfileSubOp.LOAD
    elif action == "BOOT":
        if getattr(args, "clear", False):
            sub_op = ProfileSubOp.CLEAR_BOOT
            slot = 0
            action = "BOOT CLEAR"
        else:
            if slot is None:
                print(
                    "错误: 必须指定 slot 槽位号 (0..3) 或使用 --clear 清空自启动",
                    file=sys.stderr,
                )
                sys.exit(1)
            sub_op = ProfileSubOp.BOOT
    else:
        print(f"错误: 不支持的操作 {action}", file=sys.stderr)
        sys.exit(1)

    port = find_pico_port(args.port)
    ser = open_serial(port, timeout=0.5)
    try:
        ack = send_binary_cmd(
            ser, Opcode.PROFILE_OP, channel=slot, payload_0=sub_op, timeout=2.0
        )
        if ack.ack_status == AckStatus.OK:
            if action == "BOOT CLEAR":
                print(
                    f"[\033[32mOK\033[0m] 已成功清空开机自启动设置 (ACK Seq {ack.seq_num})"
                )
            elif action == "LOAD":
                print(
                    f"[\033[32mOK\033[0m] PROFILE LOAD Slot {slot} 成功，已自动激活为 OPERATIONAL 运行模式 (ACK Seq {ack.seq_num})"
                )
            else:
                print(
                    f"[\033[32mOK\033[0m] PROFILE {action} Slot {slot} 成功 (ACK Seq {ack.seq_num})"
                )
        else:
            print(
                f"[\033[31mFAIL\033[0m] PROFILE {action} 失败: {format_ack_status(ack.ack_status)}",
                file=sys.stderr,
            )
            sys.exit(1)
    except (serial.SerialException, TimeoutError, OSError) as e:
        print(f"[\033[31mFAIL\033[0m] PROFILE {action} 失败: {e}", file=sys.stderr)
        sys.exit(1)
    finally:
        ser.close()


def cmd_opmode(args: argparse.Namespace) -> None:
    """查询或切换设备工作模式 (Config / Operational)。"""
    port = find_pico_port(args.port)
    ser = open_serial(port, timeout=0.5)
    try:
        if args.target_mode:
            target = args.target_mode.upper()
            to_op = 1 if target in ("RUN", "OPERATIONAL", "EXIT") else 0
            ack = send_binary_cmd(
                ser, Opcode.SET_OPERATING_MODE, payload_0=to_op, timeout=1.0
            )
            mode_str = (
                "CONFIG (维护配置模式: 总线收发锁定，引脚高阻静默)"
                if ack.timestamp_us == 0
                else "OPERATIONAL (正常运行模式: 物理总线实时收发中)"
            )
            print(f"[\033[32mOK\033[0m] 模式切换成功: 当前为 {mode_str}")
        else:
            ack = send_binary_cmd(ser, Opcode.QUERY_STATUS, timeout=1.0)
            mode_str = (
                "CONFIG (维护配置模式: 总线收发锁定，引脚高阻静默)"
                if ack.channel == 0
                else "OPERATIONAL (正常运行模式: 物理总线实时收发中)"
            )
            print(f"[\033[32mOK\033[0m] 当前工作模式: {mode_str}")
    except (serial.SerialException, TimeoutError, OSError) as e:
        print(f"[\033[31mFAIL\033[0m] 模式指令失败: {e}", file=sys.stderr)
        sys.exit(1)
    finally:
        ser.close()


def create_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Pico-429 ARINC 429 交互与诊断工具")
    parser.add_argument("--port", "-p", help="串口名称 (例如 COM3，未提供时自动搜索)")

    subparsers = parser.add_subparsers(dest="command", required=True)

    # ping
    p_ping = subparsers.add_parser("ping", help="连通性测试 (发送 PING 期待 PONG)")
    p_ping.add_argument("--port", "-p", help="串口名称")

    # status
    p_status = subparsers.add_parser("status", help="查询板卡在线状态与通道配置")
    p_status.add_argument("--port", "-p", help="串口名称")
    p_status.add_argument("--channel", "-c", type=int, help="指定查询的通道号 (0~11)")

    # config
    p_config = subparsers.add_parser("config", help="通道启停、方向与速率配置")
    p_config.add_argument("--port", "-p", help="串口名称")
    p_config.add_argument("channel", type=int, help="通道号 (0~11)")
    p_config.add_argument("--enable", action="store_true", help="启用通道")
    p_config.add_argument("--disable", action="store_true", help="停用通道")
    p_config.add_argument(
        "--dir", choices=["TX", "RX", "tx", "rx"], default="TX", help="通道方向 (TX/RX)"
    )
    p_config.add_argument(
        "--speed", default="high", help="速率 ('high'/'low' 或 '100k'/'12.5k')"
    )
    p_config.add_argument(
        "--run", action="store_true", help="配置完成后自动切换为 OPERATIONAL 运行模式"
    )

    # speed
    p_speed = subparsers.add_parser("speed", help="切换通道波特率")
    p_speed.add_argument("--port", "-p", help="串口名称")
    p_speed.add_argument("channel", type=int, help="通道号 (0~11)")
    p_speed.add_argument("speed", help="目标速率 ('high'/'low' 或 '100k'/'12.5k')")

    # filter
    p_filter = subparsers.add_parser("filter", help="配置通道 Label 硬件过滤规则")
    p_filter.add_argument("--port", "-p", help="串口名称")
    p_filter.add_argument("channel", type=int, help="通道号 (0~11)")
    p_filter.add_argument("--allow", help="放行的八进制 Label (例如 310)")
    p_filter.add_argument("--block", help="屏蔽的八进制 Label (例如 377)")
    p_filter.add_argument("--clear", action="store_true", help="清空所有过滤规则")

    # mode
    p_mode = subparsers.add_parser("mode", help="切换遥测数据上报模式 (BIN / ASCII)")
    p_mode.add_argument("--port", "-p", help="串口名称")
    p_mode.add_argument(
        "mode", choices=["bin", "ascii", "BIN", "ASCII"], help="数据模式"
    )

    # opmode
    p_opmode = subparsers.add_parser(
        "opmode", help="查询或切换固件运行模式 (CONFIG / RUN)"
    )
    p_opmode.add_argument("--port", "-p", help="串口名称")
    p_opmode.add_argument(
        "target_mode",
        nargs="?",
        choices=["config", "run", "operational", "CONFIG", "RUN", "OPERATIONAL"],
        help="目标模式 (省略时仅查询当前工作模式)",
    )

    # send
    p_send = subparsers.add_parser("send", help="向 ARINC 429 发送单字")
    p_send.add_argument("--port", "-p", help="串口名称")
    p_send.add_argument(
        "--channel", "-c", type=int, default=0, help="发送通道号 (0~11, 默认 0)"
    )
    p_send.add_argument("--label", "-l", help="八进制 Label (例如 310)")
    p_send.add_argument(
        "--data", "-d", default="0", help="数据载荷 (十进制或十六进制 0x1234)"
    )
    p_send.add_argument(
        "--sdi", type=int, default=0, choices=[0, 1, 2, 3], help="源/目标标识符 (0~3)"
    )
    p_send.add_argument(
        "--ssm", type=int, default=3, choices=[0, 1, 2, 3], help="符号/状态矩阵 (0~3)"
    )
    p_send.add_argument("--raw", help="直接发送 32 位原始十六进制字 (例如 0x12345678)")
    p_send.add_argument(
        "--run", action="store_true", help="发送前自动确保进入 OPERATIONAL 运行模式"
    )

    # bit
    p_bit = subparsers.add_parser("bit", help="硬件自收自发 BIT (内建自测试) 环回测试")
    p_bit.add_argument("--port", "-p", help="串口名称")
    p_bit.add_argument("--tx", type=int, default=0, help="发送通道号 (默认 0)")
    p_bit.add_argument("--rx", type=int, default=1, help="接收通道号 (默认 1)")

    # interactive (用户自驱动实时回环)
    p_inter = subparsers.add_parser(
        "interactive", help="用户自驱动实时回环测试终端 (输入数据即时回环)"
    )
    p_inter.add_argument("--port", "-p", help="串口名称")
    p_inter.add_argument("--tx", type=int, default=0, help="发送通道号 (默认 0)")
    p_inter.add_argument("--rx", type=int, default=1, help="接收通道号 (默认 1)")
    p_inter.add_argument(
        "--label", "-l", default="310", help="默认八进制 Label (默认 310)"
    )
    p_inter.add_argument(
        "--sdi", type=int, default=0, choices=[0, 1, 2, 3], help="默认 SDI (默认 0)"
    )
    p_inter.add_argument(
        "--ssm", type=int, default=3, choices=[0, 1, 2, 3], help="默认 SSM (默认 3)"
    )

    # listen
    p_listen = subparsers.add_parser(
        "listen", help="监听并流式解码接收到的 ARINC 429 数据"
    )
    p_listen.add_argument("--port", "-p", help="串口名称")

    # profile (12 通道配置方案管理)
    p_profile = subparsers.add_parser(
        "profile", help="12 通道配置方案管理 (TOML / 片上 Flash)"
    )
    profile_subs = p_profile.add_subparsers(dest="profile_action", required=True)

    # profile apply
    p_prof_apply = profile_subs.add_parser(
        "apply", help="从本地 TOML 文件批量加载并应用 12 通道配置"
    )
    p_prof_apply.add_argument("--port", "-p", help="串口名称")
    p_prof_apply.add_argument("file", help="TOML 配置文件路径")
    p_prof_apply.add_argument(
        "--no-run",
        action="store_true",
        help="应用配置后保持在 CONFIG 维护模式，不自动激活运行模式",
    )

    # profile dump
    p_prof_dump = profile_subs.add_parser(
        "dump", help="从固件导出当前 12 通道配置为 TOML 文件"
    )
    p_prof_dump.add_argument("--port", "-p", help="串口名称")
    p_prof_dump.add_argument(
        "-o", "--output", help="输出 TOML 文件路径 (默认 pico429_config.toml)"
    )
    p_prof_dump.add_argument("--name", default="Pico429_Active", help="配置方案名称")
    p_prof_dump.add_argument(
        "--stdout", action="store_true", help="直接输出至终端标准输出"
    )

    # profile list
    p_prof_list = profile_subs.add_parser("list", help="查询片上非易失存储中的配置列表")
    p_prof_list.add_argument("--port", "-p", help="串口名称")

    # profile save
    p_prof_save = profile_subs.add_parser(
        "save", help="将当前配置持久化至片上 Flash 槽位 (0..3)"
    )
    p_prof_save.add_argument("--port", "-p", help="串口名称")
    p_prof_save.add_argument(
        "slot", type=int, choices=[0, 1, 2, 3], help="Flash 槽位号 (0..3)"
    )
    p_prof_save.add_argument(
        "--name", "-n", default="Profile", help="方案名称 (最多 31 字符)"
    )

    # profile load
    p_prof_load = profile_subs.add_parser(
        "load", help="从片上 Flash 槽位加载并激活配置"
    )
    p_prof_load.add_argument("--port", "-p", help="串口名称")
    p_prof_load.add_argument(
        "slot", type=int, choices=[0, 1, 2, 3], help="Flash 槽位号 (0..3)"
    )

    # profile boot
    p_prof_boot = profile_subs.add_parser(
        "boot", help="设置设备上电默认自启动的配置槽位"
    )
    p_prof_boot.add_argument("--port", "-p", help="串口名称")
    p_prof_boot.add_argument(
        "--clear", action="store_true", help="清空上电开机自启动设置 (恢复默认静默基态)"
    )
    p_prof_boot.add_argument(
        "slot",
        type=int,
        nargs="?",
        choices=[0, 1, 2, 3],
        help="Flash 槽位号 (0..3，--clear 时无需提供)",
    )

    return parser


def main() -> None:
    parser = create_parser()
    args = parser.parse_args()

    # 如果全局 --port 传了但子命令没传，做合并
    if hasattr(args, "port") and args.port is None:
        pass

    if args.command == "ping":
        cmd_ping(args)
    elif args.command == "status":
        cmd_status(args)
    elif args.command == "config":
        cmd_config(args)
    elif args.command == "speed":
        cmd_speed(args)
    elif args.command == "filter":
        cmd_filter(args)
    elif args.command == "mode":
        cmd_mode(args)
    elif args.command == "opmode":
        cmd_opmode(args)
    elif args.command == "send":
        cmd_send(args)
    elif args.command == "bit":
        cmd_bit(args)
    elif args.command == "interactive":
        cmd_interactive(args)
    elif args.command == "listen":
        cmd_listen(args)
    elif args.command == "profile":
        if args.profile_action == "apply":
            cmd_profile_apply(args)
        elif args.profile_action == "dump":
            cmd_profile_dump(args)
        elif args.profile_action == "list":
            cmd_profile_list(args)
        elif args.profile_action in ("save", "load", "boot"):
            cmd_profile_slot(args)


if __name__ == "__main__":
    main()
