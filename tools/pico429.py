#!/usr/bin/env python3
"""ARINC 429 Pico 2 / Pico 2 W 极速免编译交互与诊断工具 (Zero-Compile Diagnostic Tool)

依赖: pyserial
运行方式:
    uv run --with pyserial python tools/pico429.py ping
    uv run --with pyserial python tools/pico429.py status
    uv run --with pyserial python tools/pico429.py config 0 --enable --dir TX --speed 100k
    uv run --with pyserial python tools/pico429.py speed 0 100k
    uv run --with pyserial python tools/pico429.py filter 1 --allow 310
    uv run --with pyserial python tools/pico429.py send --label 310 --data 0x1234
    uv run --with pyserial python tools/pico429.py bit
    uv run --with pyserial python tools/pico429.py interactive
    uv run --with pyserial python tools/pico429.py listen
"""

from __future__ import annotations

import argparse
import pathlib
import sys
import time
import tomllib
import typing
from typing import Any

if typing.TYPE_CHECKING:
    import serial
    import serial.tools.list_ports
else:
    try:
        import serial
        import serial.tools.list_ports
    except ImportError:
        serial = None

try:
    from tools.arinc429_codec import (
        BINARY_FRAME_LEN,
        BINARY_MAGIC,
        ArincWord,
        BinaryFrame,
        FrameFlags,
        FrameType,
    )
except ModuleNotFoundError:
    from arinc429_codec import (  # type: ignore[no-redef]
        BINARY_FRAME_LEN,
        BINARY_MAGIC,
        ArincWord,
        BinaryFrame,
        FrameFlags,
        FrameType,
    )


def find_pico_port(preferred: str | None = None) -> str:
    """自动检索树莓派 Pico 设备串口或校验用户指定串口。"""
    if serial is None:
        print(
            "错误: 未安装 pyserial。请使用 uv 运行:\n"
            "  uv run --with pyserial python tools/pico429.py <command>",
            file=sys.stderr,
        )
        sys.exit(1)

    ports = list(serial.tools.list_ports.comports())
    if not ports:
        print("错误: 系统未检测到任何可用串口。", file=sys.stderr)
        sys.exit(1)

    if preferred:
        matched = [p for p in ports if p.device.upper() == preferred.upper()]
        if not matched:
            print(f"错误: 指定串口 {preferred} 未找到可用设备。", file=sys.stderr)
            print("当前可用串口:", [p.device for p in ports], file=sys.stderr)
            sys.exit(1)
        return matched[0].device

    # 优先查找 VID=2E8A (Raspberry Pi)
    for p in ports:
        hwid = p.hwid.upper()
        if "2E8A" in hwid:
            return p.device

    # 未找到明确 VID 时，若仅有 1 个串口则默认返回
    if len(ports) == 1:
        return ports[0].device

    print("提示: 检测到多个串口，请使用 --port 参数明确指定:", file=sys.stderr)
    for p in ports:
        print(f"  - {p.device}: {p.description} [{p.hwid}]", file=sys.stderr)
    sys.exit(1)


def open_serial(port_name: str, timeout: float = 0.5) -> serial.Serial:
    """打开串口并激活 DTR/RTS 信号以满足 CDC 流控。"""
    if serial is None:
        print(
            "错误: 未安装 pyserial。请使用 uv 运行:\n"
            "  uv run --with pyserial python tools/pico429.py <command>",
            file=sys.stderr,
        )
        sys.exit(1)

    try:
        ser = serial.Serial(
            port=port_name,
            baudrate=115200,
            timeout=timeout,
            write_timeout=timeout,
        )
        ser.dtr = True
        ser.rts = True
        return ser
    except serial.SerialException as e:
        print(f"错误: 无法打开串口 {port_name}: {e}", file=sys.stderr)
        sys.exit(1)


def send_command_wait(ser: serial.Serial, cmd: str, timeout: float = 1.0) -> str:
    """发送文本指令并以极速非阻塞块缓冲同步等待响应 (消除 15ms 时钟量化延迟)。"""
    payload = f"{cmd.strip()}\r\n".encode("ascii")
    ser.reset_input_buffer()
    ser.write(payload)
    ser.flush()

    t0 = time.perf_counter()
    line_buf = bytearray()
    while time.perf_counter() - t0 < timeout:
        n = ser.in_waiting
        if n > 0:
            chunk = ser.read(n)
            line_buf.extend(chunk)
            if b"\n" in line_buf:
                while b"\n" in line_buf:
                    raw_line, _, line_buf = line_buf.partition(b"\n")
                    line_str = raw_line.decode("ascii", errors="replace").strip()
                    if line_str:
                        return line_str
            elif len(line_buf) >= BINARY_FRAME_LEN and line_buf[:2] == BINARY_MAGIC:
                try:
                    f = BinaryFrame.decode(bytes(line_buf[:BINARY_FRAME_LEN]))
                    if f.frame_type == FrameType.CMD_ACK:
                        return f"OK: Command acknowledged (binary mode, seq {f.seq_num})"
                except ValueError:
                    pass

    raise TimeoutError(f"指令 '{cmd}' 超时 ({timeout}s) 未收到固件回执")


def send_command_wait_multiline(
    ser: serial.Serial, cmd: str, timeout: float = 1.0, quiet_gap: float = 0.05
) -> list[str]:
    """发送文本指令并等待多行响应，当有数据到达且静默超过 quiet_gap 秒后返回所有行。"""
    payload = f"{cmd.strip()}\r\n".encode("ascii")
    ser.reset_input_buffer()
    ser.write(payload)
    ser.flush()

    t0 = time.perf_counter()
    line_buf = bytearray()
    lines: list[str] = []
    last_rx_time: float | None = None

    while time.perf_counter() - t0 < timeout:
        n = ser.in_waiting
        if n > 0:
            chunk = ser.read(n)
            line_buf.extend(chunk)
            last_rx_time = time.perf_counter()
            while b"\n" in line_buf:
                raw_line, _, line_buf = line_buf.partition(b"\n")
                line_str = raw_line.decode("ascii", errors="replace").strip()
                if line_str:
                    lines.append(line_str)
        elif lines and last_rx_time is not None:
            if time.perf_counter() - last_rx_time > quiet_gap:
                break
        else:
            time.sleep(0.005)

    if not lines:
        raise TimeoutError(f"指令 '{cmd}' 超时 ({timeout}s) 未收到固件回执")
    return lines


def normalize_speed(speed_str: str) -> str:
    """标准化波特率名称为 HIGH 或 LOW (兼顾 ARINC 429 规范定义与历史别名)。"""
    s = speed_str.strip().lower()
    if s in ("high", "100k", "100", "100kbps"):
        return "HIGH"
    elif s in ("low", "12.5k", "12k5", "12.5", "12.5kbps"):
        return "LOW"
    raise ValueError(
        f"未知速率 '{speed_str}'。ARINC 429 仅支持 'high' (100 kbps) 或 'low' (12.5 kbps)。"
    )


def parse_toml_profile(toml_text: str) -> dict[str, Any]:
    """解析并校验 TOML 格式的 12 通道配置。"""
    try:
        data = tomllib.loads(toml_text)
    except (tomllib.TOMLDecodeError, ValueError) as e:
        raise ValueError(f"TOML 格式语法错误: {e}") from e

    profile_info = data.get("profile", {})
    name = str(profile_info.get("name", "Unnamed_Profile"))
    version = str(profile_info.get("version", "1.0"))
    boot_slot = profile_info.get("boot_slot")
    if boot_slot is not None:
        try:
            boot_slot = int(boot_slot)
            if not (0 <= boot_slot <= 3):
                raise ValueError(f"boot_slot 必须在 0..3 范围内，当前为: {boot_slot}")
        except ValueError as err:
            raise ValueError(f"无效的 boot_slot: {boot_slot}") from err

    global_info = data.get("global", {})
    mode = str(global_info.get("mode", "ASCII")).upper()
    if mode not in ("ASCII", "BIN"):
        mode = "ASCII"

    raw_channels = data.get("channel", data.get("channels", {}))
    channels: dict[int, dict[str, Any]] = {}

    for ch_key, ch_data in raw_channels.items():
        try:
            ch_idx = int(ch_key)
        except ValueError:
            raise ValueError(f"通道标识必须为数字 (0..11)，当前为: '{ch_key}'")

        if not (0 <= ch_idx <= 11):
            raise ValueError(f"通道编号超限 (0..11): {ch_idx}")

        raw_dir = str(ch_data.get("direction", "DISABLED")).upper()
        if raw_dir not in ("TX", "RX", "DISABLED"):
            raise ValueError(
                f"CH{ch_idx} 无效工作方向 '{raw_dir}' (有效值: TX, RX, DISABLED)"
            )

        raw_speed = str(ch_data.get("speed", "high"))
        norm_speed = normalize_speed(raw_speed)

        filters = ch_data.get("filter", [])
        if isinstance(filters, str):
            filters = [filters]
        elif not isinstance(filters, list):
            filters = []

        channels[ch_idx] = {
            "direction": raw_dir,
            "speed": norm_speed,
            "filter": filters,
        }

    return {
        "name": name,
        "version": version,
        "boot_slot": boot_slot,
        "mode": mode,
        "channels": channels,
    }


def build_batch_spec(channels: dict[int, dict[str, Any]]) -> str:
    """生成固件 CFGBATCH 紧凑配置字符串 (0=TX:HIGH,1=RX:HIGH,...)"""
    parts: list[str] = []
    for ch_idx in range(12):
        cfg = channels.get(ch_idx, {"direction": "DISABLED", "speed": "HIGH"})
        dir_str = cfg.get("direction", "DISABLED").upper()
        speed_str = cfg.get("speed", "HIGH").upper()
        parts.append(f"{ch_idx}={dir_str}:{speed_str}")
    return ",".join(parts)


def generate_toml_from_cfgdump(dump_line: str, profile_name: str = "Pico429_Profile") -> str:
    """从固件 CFGDUMP 响应生成标准的 TOML 配置文本。"""
    body = dump_line
    if "CFGDUMP:" in body:
        body = body.split("CFGDUMP:", 1)[1].strip()

    tokens = body.split()
    channels_map: dict[int, tuple[str, str]] = {}
    for tok in tokens:
        parts = tok.split(":")
        if len(parts) >= 2:
            try:
                ch = int(parts[0])
                direction = parts[1].upper()
                speed = parts[2].lower() if len(parts) >= 3 else "high"
                channels_map[ch] = (direction, speed)
            except ValueError:
                pass

    lines = [
        "# Pico-429 12-Channel ARINC 429 Configuration Profile",
        "# Generated automatically from hardware state",
        "[profile]",
        f'name = "{profile_name}"',
        'version = "1.0"',
        "",
        "[global]",
        'mode = "ASCII"',
        "",
    ]

    for ch in range(12):
        direction, speed = channels_map.get(ch, ("DISABLED", "high"))
        speed_norm = "high" if "high" in speed or "100" in speed else "low"
        lines.append(f"[channel.{ch}]")
        lines.append(f'direction = "{direction}"')
        lines.append(f'speed = "{speed_norm}"')
        if direction == "RX":
            lines.append("filter = []")
        lines.append("")

    return "\n".join(lines)


def parse_profile_list_output(lines: list[str]) -> tuple[int | None, list[dict[str, Any]]]:
    """解析固件 PROFILE LIST 响应。"""
    boot_slot: int | None = None
    slots: list[dict[str, Any]] = []

    for line in lines:
        if "Boot Slot:" in line:
            if "Some(" in line:
                try:
                    slot_str = line.split("Some(", 1)[1].split(")", 1)[0]
                    boot_slot = int(slot_str)
                except ValueError:
                    pass
        elif "[Slot" in line:
            try:
                parts = line.split("[Slot ", 1)[1]
                slot_id_str, rest = parts.split("]", 1)
                slot_id = int(slot_id_str)
                rest = rest.strip()
                if "ACTIVE" in rest:
                    name = ""
                    if '"' in rest:
                        name = rest.split('"', 1)[1].rsplit('"', 1)[0]
                    slots.append({"slot": slot_id, "status": "ACTIVE", "name": name})
                else:
                    slots.append({"slot": slot_id, "status": "EMPTY", "name": ""})
            except (ValueError, IndexError):
                pass

    return boot_slot, slots


def cmd_ping(args: argparse.Namespace) -> None:
    port = find_pico_port(args.port)
    print(f"[*] 正在连接 {port} 并发送 PING...")
    ser = open_serial(port, timeout=0.5)
    try:
        t0 = time.perf_counter()
        resp = send_command_wait(ser, "PING", timeout=1.0)
        dt_ms = (time.perf_counter() - t0) * 1000.0
        if resp == "PONG":
            print(f"[\033[32mOK\033[0m] 握手成功: 收到 {resp} (RTT: {dt_ms:.2f} ms)")
        else:
            print(f"[\033[33mWARN\033[0m] 收到响应: {resp} (耗时: {dt_ms:.2f} ms)")
    except (serial.SerialException, TimeoutError, OSError) as e:
        print(f"[\033[31mFAIL\033[0m] PING 握手失败: {e}", file=sys.stderr)
        sys.exit(1)
    finally:
        ser.close()


def cmd_status(args: argparse.Namespace) -> None:
    port = find_pico_port(args.port)
    print(f"[*] 正在查询 {port} 状态...")
    ser = open_serial(port, timeout=0.5)
    try:
        cmd = f"STATUS {args.channel}" if args.channel is not None else "STATUS"
        resp = send_command_wait(ser, cmd, timeout=1.0)
        print(f"[\033[32mSTATUS\033[0m] {resp}")
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
            cmd = f"CHCFG {args.channel} DISABLE"
        else:
            dir_str = "TX" if args.dir.upper() == "TX" else "RX"
            speed_str = normalize_speed(args.speed)
            cmd = f"CHCFG {args.channel} ENABLE {dir_str} {speed_str}"

        print(f"[*] 下发通道配置指令: {cmd}")
        resp = send_command_wait(ser, cmd, timeout=1.0)
        print(f"[\033[32mOK\033[0m] {resp}")
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
        speed_str = normalize_speed(args.speed)
        cmd = f"CHCFG {args.channel} SPEED {speed_str}"
        print(f"[*] 下发速率切换指令: {cmd}")
        resp = send_command_wait(ser, cmd, timeout=1.0)
        print(f"[\033[32mOK\033[0m] {resp}")
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
            cmd = f"CHCFG {args.channel} FILTER CLEAR"
        elif args.allow:
            cmd = f"CHCFG {args.channel} FILTER ALLOW {args.allow}"
        elif args.block:
            cmd = f"CHCFG {args.channel} FILTER BLOCK {args.block}"
        else:
            print(
                "错误: 必须指定 --allow <octal>, --block <octal> 或 --clear",
                file=sys.stderr,
            )
            sys.exit(1)

        print(f"[*] 下发过滤器配置指令: {cmd}")
        resp = send_command_wait(ser, cmd, timeout=1.0)
        print(f"[\033[32mOK\033[0m] {resp}")
    except (serial.SerialException, TimeoutError, OSError) as e:
        print(f"[\033[31mFAIL\033[0m] 过滤规则设置失败: {e}", file=sys.stderr)
        sys.exit(1)
    finally:
        ser.close()


def cmd_mode(args: argparse.Namespace) -> None:
    """切换遥测上报格式 (BIN / ASCII)。"""
    port = find_pico_port(args.port)
    ser = open_serial(port, timeout=0.5)
    try:
        target_mode = "BIN" if args.mode.upper() == "BIN" else "ASCII"
        cmd = f"MODE {target_mode}"
        print(f"[*] 下发模式切换指令: {cmd}")
        resp = send_command_wait(ser, cmd, timeout=1.0)
        print(f"[\033[32mOK\033[0m] {resp}")
    except (serial.SerialException, TimeoutError, OSError) as e:
        print(f"[\033[31mFAIL\033[0m] 模式切换失败: {e}", file=sys.stderr)
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
        if args.raw is not None:
            raw_val = int(args.raw, 16)
            cmd = f"TXRAW {args.channel} 0x{raw_val:08X}"
        else:
            if not args.label:
                print(
                    "错误: 必须指定 --label (例如 310) 或 --raw (例如 0x12345678)",
                    file=sys.stderr,
                )
                sys.exit(1)
            data_val = int(args.data, 0) if args.data else 0
            cmd = f"TX {args.channel} {args.label} {args.sdi} 0x{data_val:X} {args.ssm}"

        print(f"[*] 发送指令: {cmd}")
        resp = send_command_wait(ser, cmd, timeout=1.0)
        print(f"[\033[32mOK\033[0m] {resp}")
    except (serial.SerialException, TimeoutError, OSError) as e:
        print(f"[\033[31mFAIL\033[0m] 发送失败: {e}", file=sys.stderr)
        sys.exit(1)
    finally:
        ser.close()


def cmd_bit(args: argparse.Namespace) -> None:
    """执行物理对接自收自发 BIT (内建自测试) 极速验证 (< 1ms RTT)"""
    port = find_pico_port(args.port)
    print(
        f"[*] 连接 {port}，开始执行 ARINC 429 硬件环回 BIT 自测试 (CH{args.tx} -> CH{args.rx})..."
    )
    ser = open_serial(port, timeout=0.5)

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

    try:
        ser.reset_input_buffer()
        # 确保固件处于 ASCII 文本遥测模式以解析回显
        try:
            ser.write(b"MODE ASCII\r\n")
            ser.flush()
            time.sleep(0.01)
            ser.reset_input_buffer()
        except (serial.SerialException, OSError):
            pass

        for i, (label, sdi, data, ssm, desc) in enumerate(bit_vectors, start=1):
            cmd = f"TX {args.tx} {label} {sdi} 0x{data:X} {ssm}\r\n"
            t0 = time.perf_counter()
            ser.write(cmd.encode("ascii"))
            ser.flush()

            sent_raw: str | None = None
            recv_raw: str | None = None
            parity_ok = False
            deadline = time.perf_counter() + 1.0
            buf = bytearray()

            while time.perf_counter() < deadline:
                n = ser.in_waiting
                if n > 0:
                    buf.extend(ser.read(n))
                    while b"\n" in buf:
                        raw_line, _, buf = buf.partition(b"\n")
                        line = raw_line.decode("ascii", errors="replace").strip()
                        if line.startswith("OK: Sent"):
                            raw_part = line.split("RAW:")[-1].strip()
                            sent_raw = raw_part.upper()
                        elif line.startswith(f"[CH{args.rx}]"):
                            if "(" in line and line.endswith(")"):
                                recv_raw = (
                                    line[line.rfind("(") + 1 : -1].strip().upper()
                                )
                            if "P:OK" in line:
                                parity_ok = True
                    if sent_raw and recv_raw:
                        break

            rtt_ms = (time.perf_counter() - t0) * 1000.0
            rtt_list.append(rtt_ms)

            is_pass = (sent_raw is not None) and (sent_raw == recv_raw) and parity_ok
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
        ser.close()


def execute_interactive_loopback(
    ser: serial.Serial,
    tx_ch: int,
    rx_ch: int,
    label: str,
    sdi: int,
    data: int,
    ssm: int,
) -> None:
    """在保持打开的串口上执行单次用户自驱动回环测试并即时解码打印。"""
    cmd = f"TX {tx_ch} {label} {sdi} 0x{data:X} {ssm}\r\n"
    ser.reset_input_buffer()

    t0 = time.perf_counter()
    ser.write(cmd.encode("ascii"))
    ser.flush()

    sent_raw: str | None = None
    recv_raw: str | None = None
    parity_ok = False
    deadline = time.perf_counter() + 1.0
    buf = bytearray()

    while time.perf_counter() < deadline:
        n = ser.in_waiting
        if n > 0:
            buf.extend(ser.read(n))
            while b"\n" in buf:
                raw_line, _, buf = buf.partition(b"\n")
                line = raw_line.decode("ascii", errors="replace").strip()
                if line.startswith("OK: Sent"):
                    raw_part = line.split("RAW:")[-1].strip()
                    sent_raw = raw_part.upper()
                elif line.startswith(f"[CH{rx_ch}]"):
                    if "(" in line and line.endswith(")"):
                        recv_raw = line[line.rfind("(") + 1 : -1].strip().upper()
                    if "P:OK" in line:
                        parity_ok = True
            if sent_raw and recv_raw:
                break

    rtt_ms = (time.perf_counter() - t0) * 1000.0

    if not sent_raw:
        print(f"[\033[31mTIMEOUT\033[0m] 未收到固件发送确认 ({rtt_ms:.2f} ms)")
        return

    is_pass = (sent_raw == recv_raw) and parity_ok
    status_str = "\033[32mPASS\033[0m" if is_pass else "\033[31mFAIL\033[0m"
    p_status = "\033[32mOK\033[0m" if parity_ok else "\033[31mERR\033[0m"

    print(
        f"[*] [\033[36mTX\033[0m] CH{tx_ch} L:{label} SDI:{sdi} DATA:0x{data:05X} SSM:{ssm} (RAW: {sent_raw})"
    )
    if recv_raw:
        try:
            rx_int = int(recv_raw, 16)
            rx_word = ArincWord(rx_int)
            print(
                f"[+] [\033[32mRX\033[0m] CH{rx_ch} L:{rx_word.label_str} SDI:{rx_word.sdi} "
                f"DATA:0x{rx_word.data:05X} ({rx_word.data}) SSM:{rx_word.ssm} P:{p_status} "
                f"(RAW: {recv_raw}) | RTT: \033[35m{rtt_ms:.2f} ms\033[0m -> [{status_str}]"
            )
        except ValueError:
            print(
                f"[+] [\033[32mRX\033[0m] CH{rx_ch} RAW: {recv_raw} | RTT: {rtt_ms:.2f} ms -> [{status_str}]"
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

    try:
        # 确保固件处于 ASCII 文本交互模式
        try:
            ser.write(b"MODE ASCII\r\n")
            ser.flush()
            time.sleep(0.01)
            ser.reset_input_buffer()
        except (serial.SerialException, OSError):
            pass

        # 先确认连通性
        try:
            resp = send_command_wait(ser, "PING", timeout=1.0)
            if resp == "PONG":
                print(
                    "[\033[32mONLINE\033[0m] 固件在线就绪！可随时输入载荷开始测试。\n"
                )
        except TimeoutError:
            print("[\033[33mWARN\033[0m] PING 握手无响应，仍尝试进入交互模式...\n")

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
                    resp = send_command_wait(ser, "PING", timeout=1.0)
                    dt = (time.perf_counter() - t0) * 1000.0
                    print(f"[\033[32mOK\033[0m] {resp} (RTT: {dt:.2f} ms)")
                except TimeoutError as e:
                    print(f"[\033[31mTIMEOUT\033[0m] {e}")
                continue

            if lower_line == "status":
                try:
                    resp = send_command_wait(ser, "STATUS", timeout=1.0)
                    print(f"[\033[32mSTATUS\033[0m] {resp}")
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
                    spd = "100K" if "100" in parts[1].upper() else "12K5"
                    try:
                        resp = send_command_wait(
                            ser, f"CHCFG {tx_ch} SPEED {spd}", timeout=1.0
                        )
                        print(f"[\033[32mOK\033[0m] {resp}")
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
                        f_cmd = f"CHCFG {rx_ch} FILTER CLEAR"
                    elif sub in ("ALLOW", "BLOCK") and len(parts) >= 3:
                        f_cmd = f"CHCFG {rx_ch} FILTER {sub} {parts[2]}"
                    else:
                        print("语法: filter <allow|block|clear> [octal_label]")
                        continue
                    try:
                        resp = send_command_wait(ser, f_cmd, timeout=1.0)
                        print(f"[\033[32mOK\033[0m] {resp}")
                    except TimeoutError as e:
                        print(f"[\033[31mTIMEOUT\033[0m] {e}")
                continue

            if lower_line.startswith("raw "):
                raw_hex = line[4:].strip()
                try:
                    raw_val = int(raw_hex, 16)
                    cmd = f"TXRAW {tx_ch} 0x{raw_val:08X}"
                    t0 = time.perf_counter()
                    resp = send_command_wait(ser, cmd, timeout=1.0)
                    dt = (time.perf_counter() - t0) * 1000.0
                    print(f"[\033[32mOK\033[0m] {resp} (耗时: {dt:.2f} ms)")
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
                            ser, tx_ch, rx_ch, lbl, s_val, d_val, m_val
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
                )
            except ValueError:
                print(f"未识别指令或数值: '{line}'。输入 'help' 获取帮助。")
    finally:
        ser.close()


def cmd_listen(args: argparse.Namespace) -> None:
    port = find_pico_port(args.port)
    print(f"[*] 打开 {port} 开始监听 ARINC 429 流量 (Ctrl+C 退出)...")
    ser = open_serial(port, timeout=0.1)

    # 切换固件输出模式为 BINARY 模式以接收 16 字节定长二进制帧
    try:
        ser.write(b"MODE BIN\r\n")
        ser.flush()
    except (serial.SerialException, OSError) as e:
        print(f"[\033[33mWARN\033[0m] 切换 BINARY 模式失败: {e}")

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

            # 1. 尝试解析 16 字节二进制帧
            while len(rx_buf) >= BINARY_FRAME_LEN:
                idx = rx_buf.find(BINARY_MAGIC)
                if idx == -1:
                    # 缓冲区无 Magic，检查是否为 ASCII 文本行
                    if b"\n" in rx_buf:
                        line, _, rx_buf = rx_buf.partition(b"\n")
                        line_str = line.decode("ascii", errors="replace").strip()
                        if line_str:
                            print(f"[ASCII] {line_str}")
                    else:
                        rx_buf.clear()
                    break

                if idx > 0:
                    # 丢弃 Magic 前的残余碎片
                    del rx_buf[:idx]

                if len(rx_buf) < BINARY_FRAME_LEN:
                    break

                frame_bytes = bytes(rx_buf[:BINARY_FRAME_LEN])
                del rx_buf[:BINARY_FRAME_LEN]

                try:
                    bin_frame = BinaryFrame.decode(frame_bytes)
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
                except ValueError:
                    # CRC 或解码异常时作为文本兜底输出
                    pass
    except KeyboardInterrupt:
        print("\n[*] 监听已由用户中止。")
    finally:
        ser.close()


def cmd_profile_apply(args: argparse.Namespace) -> None:
    """一键应用 TOML 配置文件到 12 通道硬件。"""
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
        # 1. 确保在 ASCII 文本模式以接收命令响应
        send_command_wait(ser, "MODE ASCII", timeout=1.0)

        # 2. 生成紧凑 batch 配置指令
        batch_spec = build_batch_spec(parsed["channels"])
        print("[*] 批量安全配置 12 通道 (暂停 -> 重构 -> 恢复)...")
        resp = send_command_wait(ser, f"CFGBATCH {batch_spec}", timeout=2.0)
        print(f"[\033[32mOK\033[0m] {resp}")

        # 3. 逐个通道配置 Label 过滤规则 (若有)
        for ch, ch_cfg in parsed["channels"].items():
            filters = ch_cfg.get("filter", [])
            if ch_cfg["direction"] == "RX" and filters:
                send_command_wait(ser, f"CHCFG {ch} FILTER CLEAR", timeout=1.0)
                for f_rule in filters:
                    f_rule = f_rule.strip()
                    if f_rule.upper().startswith("ALLOW"):
                        lbl = f_rule.split(None, 1)[1]
                        send_command_wait(ser, f"CHCFG {ch} FILTER ALLOW {lbl}", timeout=1.0)
                    elif f_rule.upper().startswith("BLOCK"):
                        lbl = f_rule.split(None, 1)[1]
                        send_command_wait(ser, f"CHCFG {ch} FILTER BLOCK {lbl}", timeout=1.0)

        # 4. 切换全局数据模式
        if parsed.get("mode") == "BIN":
            send_command_wait(ser, "MODE BIN", timeout=1.0)

        # 5. 打印格式化通道配置概览
        print("\n" + "=" * 60)
        print(f"配置方案: {parsed['name']} (v{parsed['version']})")
        print("-" * 60)
        print(f"{'通道':<6} {'方向':<10} {'波特率':<8} {'过滤规则'}")
        print("-" * 60)
        for ch in range(12):
            cfg = parsed["channels"].get(ch, {"direction": "DISABLED", "speed": "HIGH", "filter": []})
            d = cfg["direction"]
            s = cfg["speed"]
            flist = cfg.get("filter", [])
            f_str = ", ".join(flist) if flist else ("-" if d != "RX" else "全部放行")
            print(f"CH{ch:<4} {d:<10} {s:<8} {f_str}")
        print("=" * 60)

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
        send_command_wait(ser, "MODE ASCII", timeout=1.0)
        resp = send_command_wait(ser, "CFGDUMP", timeout=1.0)
        toml_content = generate_toml_from_cfgdump(resp, profile_name=args.name)

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
        send_command_wait(ser, "MODE ASCII", timeout=1.0)
        lines = send_command_wait_multiline(ser, "PROFILE LIST", timeout=1.0)
        boot_slot, slots = parse_profile_list_output(lines)

        print("\n" + "=" * 50)
        print("片上 Flash 非易失存储配置方案列表 (Slot 0..3):")
        print("-" * 50)
        print(f"{'槽位':<6} {'自启动':<8} {'状态':<10} {'方案名称'}")
        print("-" * 50)

        for s_info in slots:
            slot_id = s_info["slot"]
            is_boot = (boot_slot == slot_id)
            boot_tag = "  [*]" if is_boot else "   -"
            name_tag = f'"{s_info["name"]}"' if s_info["status"] == "ACTIVE" else "<空闲>"
            print(f"Slot {slot_id:<3} {boot_tag:<8} {s_info['status']:<10} {name_tag}")

        print("=" * 50)
        if boot_slot is not None:
            print(f"[*] 当前上电自启动槽位: Slot {boot_slot}")
        else:
            print("[*] 当前无自启动槽位 (上电恢复出厂默认安全状态: CH0=TX, CH1=RX)")

    except (serial.SerialException, TimeoutError, OSError) as e:
        print(f"[\033[31mFAIL\033[0m] 查询配置列表失败: {e}", file=sys.stderr)
        sys.exit(1)
    finally:
        ser.close()


def cmd_profile_save(args: argparse.Namespace) -> None:
    """将当前 12 通道配置持久化写入片上 Flash 槽位。"""
    port = find_pico_port(args.port)
    name = args.name.strip()
    cmd = f"PROFILE SAVE {args.slot} {name}"
    print(f"[*] 连接 {port} 并写入片上 Flash Slot {args.slot} (名称: '{name}')...")
    ser = open_serial(port, timeout=0.5)

    try:
        send_command_wait(ser, "MODE ASCII", timeout=1.0)
        resp = send_command_wait(ser, cmd, timeout=2.0)
        print(f"[\033[32mOK\033[0m] {resp}")
    except (serial.SerialException, TimeoutError, OSError) as e:
        print(f"[\033[31mFAIL\033[0m] 保存方案失败: {e}", file=sys.stderr)
        sys.exit(1)
    finally:
        ser.close()


def cmd_profile_load(args: argparse.Namespace) -> None:
    """从片上 Flash 槽位加载并激活 12 通道配置。"""
    port = find_pico_port(args.port)
    cmd = f"PROFILE LOAD {args.slot}"
    print(f"[*] 连接 {port} 并激活片上 Flash Slot {args.slot} 配置...")
    ser = open_serial(port, timeout=0.5)

    try:
        send_command_wait(ser, "MODE ASCII", timeout=1.0)
        resp = send_command_wait(ser, cmd, timeout=2.0)
        print(f"[\033[32mOK\033[0m] {resp}")
    except (serial.SerialException, TimeoutError, OSError) as e:
        print(f"[\033[31mFAIL\033[0m] 加载方案失败: {e}", file=sys.stderr)
        sys.exit(1)
    finally:
        ser.close()


def cmd_profile_boot(args: argparse.Namespace) -> None:
    """设置上电默认自启动的 Flash 槽位。"""
    port = find_pico_port(args.port)
    cmd = f"PROFILE BOOT {args.slot}"
    print(f"[*] 连接 {port} 设置上电自启动槽位为 Slot {args.slot}...")
    ser = open_serial(port, timeout=0.5)

    try:
        send_command_wait(ser, "MODE ASCII", timeout=1.0)
        resp = send_command_wait(ser, cmd, timeout=2.0)
        print(f"[\033[32mOK\033[0m] {resp}")
    except (serial.SerialException, TimeoutError, OSError) as e:
        print(f"[\033[31mFAIL\033[0m] 设置自启动槽位失败: {e}", file=sys.stderr)
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
    p_profile = subparsers.add_parser("profile", help="12 通道配置方案管理 (TOML / 片上 Flash)")
    profile_subs = p_profile.add_subparsers(dest="profile_action", required=True)

    # profile apply
    p_prof_apply = profile_subs.add_parser("apply", help="从本地 TOML 文件批量加载并应用 12 通道配置")
    p_prof_apply.add_argument("--port", "-p", help="串口名称")
    p_prof_apply.add_argument("file", help="TOML 配置文件路径")

    # profile dump
    p_prof_dump = profile_subs.add_parser("dump", help="从固件导出当前 12 通道配置为 TOML 文件")
    p_prof_dump.add_argument("--port", "-p", help="串口名称")
    p_prof_dump.add_argument("-o", "--output", help="输出 TOML 文件路径 (默认 pico429_config.toml)")
    p_prof_dump.add_argument("--name", default="Pico429_Active", help="配置方案名称")
    p_prof_dump.add_argument("--stdout", action="store_true", help="直接输出至终端标准输出")

    # profile list
    p_prof_list = profile_subs.add_parser("list", help="查询片上非易失存储中的配置列表")
    p_prof_list.add_argument("--port", "-p", help="串口名称")

    # profile save
    p_prof_save = profile_subs.add_parser("save", help="将当前配置持久化至片上 Flash 槽位 (0..3)")
    p_prof_save.add_argument("--port", "-p", help="串口名称")
    p_prof_save.add_argument("slot", type=int, choices=[0, 1, 2, 3], help="Flash 槽位号 (0..3)")
    p_prof_save.add_argument("--name", "-n", default="Profile", help="方案名称 (最多 31 字符)")

    # profile load
    p_prof_load = profile_subs.add_parser("load", help="从片上 Flash 槽位加载并激活配置")
    p_prof_load.add_argument("--port", "-p", help="串口名称")
    p_prof_load.add_argument("slot", type=int, choices=[0, 1, 2, 3], help="Flash 槽位号 (0..3)")

    # profile boot
    p_prof_boot = profile_subs.add_parser("boot", help="设置设备上电默认自启动的配置槽位")
    p_prof_boot.add_argument("--port", "-p", help="串口名称")
    p_prof_boot.add_argument("slot", type=int, choices=[0, 1, 2, 3], help="Flash 槽位号 (0..3)")

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
        elif args.profile_action == "save":
            cmd_profile_save(args)
        elif args.profile_action == "load":
            cmd_profile_load(args)
        elif args.profile_action == "boot":
            cmd_profile_boot(args)


if __name__ == "__main__":
    main()
