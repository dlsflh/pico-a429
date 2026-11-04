#!/usr/bin/env python3
"""ARINC 429 Pico 2 / Pico 2 W 极简免编译交互工具 (Zero-Compile Diagnostic Tool)

依赖: pyserial
运行方式:
    uv run --with pyserial python tools/pico429.py ping
    uv run --with pyserial python tools/pico429.py status
    uv run --with pyserial python tools/pico429.py send --label 310 --data 0x1234
    uv run --with pyserial python tools/pico429.py listen
"""

from __future__ import annotations

import argparse
import sys
import time

try:
    import serial
    import serial.tools.list_ports
except ImportError:
    print(
        "错误: 未安装 pyserial。请使用 uv 运行:\n"
        "  uv run --with pyserial python tools/pico429.py <command>",
        file=sys.stderr,
    )
    sys.exit(1)

MAGIC_BYTES = bytes([0x41, 0x29])
BINARY_FRAME_LEN = 16


def find_pico_port(preferred: str | None = None) -> str:
    """自动检索树莓派 Pico 设备串口或校验用户指定串口。"""
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


def open_serial(port_name: str, timeout: float = 1.0) -> serial.Serial:
    """打开串口并强制激活 DTR/RTS 信号以满足 CDC 流控。"""
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
    """发送文本指令并同步等待响应 (超时抛出异常)。"""
    payload = f"{cmd.strip()}\r\n".encode("ascii")
    ser.reset_input_buffer()
    ser.write(payload)
    ser.flush()

    start = time.time()
    line_buf = bytearray()
    while time.time() - start < timeout:
        if ser.in_waiting > 0:
            b = ser.read(1)
            if not b:
                continue
            if b == b"\n":
                line_str = line_buf.decode("ascii", errors="replace").strip()
                if line_str:
                    return line_str
                line_buf.clear()
            elif b != b"\r":
                line_buf.extend(b)
        else:
            time.sleep(0.01)

    raise TimeoutError(f"指令 '{cmd}' 超时 ({timeout}s) 未收到固件回执")


def cmd_ping(args: argparse.Namespace) -> None:
    port = find_pico_port(args.port)
    print(f"[*] 正在连接 {port} 并发送 PING...")
    ser = open_serial(port, timeout=1.0)
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
    ser = open_serial(port, timeout=1.0)
    try:
        cmd = f"STATUS {args.channel}" if args.channel is not None else "STATUS"
        resp = send_command_wait(ser, cmd, timeout=1.0)
        print(f"[\033[32mSTATUS\033[0m] {resp}")
    except (serial.SerialException, TimeoutError, OSError) as e:
        print(f"[\033[31mFAIL\033[0m] 查询状态失败: {e}", file=sys.stderr)
        sys.exit(1)
    finally:
        ser.close()


def cmd_send(args: argparse.Namespace) -> None:
    port = find_pico_port(args.port)
    ser = open_serial(port, timeout=1.0)
    try:
        if args.raw is not None:
            raw_val = int(args.raw, 16)
            cmd = f"TXRAW {args.channel} 0x{raw_val:08X}"
        else:
            if not args.label:
                print("错误: 必须指定 --label (例如 310) 或 --raw (例如 0x12345678)", file=sys.stderr)
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
    """执行物理对接自收自发 BIT (内建自测试) 验证"""
    port = find_pico_port(args.port)
    print(f"[*] 连接 {port}，开始执行 ARINC 429 硬件环回 BIT 自测试 (CH{args.tx} -> CH{args.rx})...")
    ser = open_serial(port, timeout=1.0)

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
    rtt_list = []

    try:
        ser.reset_input_buffer()
        time.sleep(0.05)

        for i, (label, sdi, data, ssm, desc) in enumerate(bit_vectors, start=1):
            cmd = f"TX {args.tx} {label} {sdi} 0x{data:X} {ssm}\r\n"
            t0 = time.perf_counter()
            ser.write(cmd.encode("ascii"))
            ser.flush()

            sent_raw = None
            recv_raw = None
            parity_ok = False
            start = time.time()

            while time.time() - start < 1.0:
                line = ser.readline().decode("ascii", errors="replace").strip()
                if not line:
                    continue
                if line.startswith("OK: Sent"):
                    # 提取发送端字: RAW:0xXXXXXXXX
                    raw_part = line.split("RAW:")[-1].strip()
                    sent_raw = raw_part.upper()
                elif line.startswith(f"[CH{args.rx}]"):
                    # 提取接收端字: (0xXXXXXXXX)
                    if "(" in line and line.endswith(")"):
                        recv_raw = line[line.rfind("(") + 1 : -1].strip().upper()
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
                f"#{i:<3} {desc:<20} {tx_info:<20} {sent_disp:<14} {recv_disp:<14} {rtt_ms:<6.1f}ms {status_str}"
            )
            time.sleep(0.02)

        print("=" * 102)
        avg_rtt = sum(rtt_list) / len(rtt_list) if rtt_list else 0.0
        loss_rate = ((total_count - passed_count) / total_count) * 100.0

        if passed_count == total_count:
            print(
                f"[\033[32mBIT PASSED\033[0m] {passed_count}/{total_count} 向量全通过 | "
                f"丢包率: {loss_rate:.1f}% | 平均 RTT: {avg_rtt:.2f} ms | "
                f"\033[32m硬件物理收发完全正常！\033[0m"
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


def reverse_label_bits(label_byte: int) -> int:
    """ARINC 429 Label 规范：总线位倒序还原为八进制 Label。"""
    res = 0
    for i in range(8):
        if (label_byte >> i) & 1:
            res |= 1 << (7 - i)
    return res


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
            chunk = ser.read(ser.in_waiting or 1)
            if not chunk:
                continue
            rx_buf.extend(chunk)

            # 1. 尝试解析 16 字节二进制帧
            while len(rx_buf) >= BINARY_FRAME_LEN:
                idx = rx_buf.find(MAGIC_BYTES)
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

                # 解包字段
                type_ch = frame_bytes[2]
                ch = type_ch & 0x0F
                flags = frame_bytes[3]
                seq = int.from_bytes(frame_bytes[4:6], "little")
                ts_us = int.from_bytes(frame_bytes[6:10], "little")
                raw_word = int.from_bytes(frame_bytes[10:14], "little")

                # 解析 32-bit ARINC 字
                raw_label = raw_word & 0xFF
                oct_label = reverse_label_bits(raw_label)
                sdi = (raw_word >> 8) & 0x03
                data = (raw_word >> 10) & 0x7FFFF
                ssm = (raw_word >> 29) & 0x03
                parity_valid = raw_word.bit_count() % 2 == 1
                p_str = "\033[32mOK\033[0m" if parity_valid else "\033[31mERR\033[0m"
                speed_str = "100K" if (flags & (1 << 1)) else "12.5K"

                print(
                    f"CH{ch:<4} #{seq:<5} {ts_us:<12} {speed_str:<6} {oct_label:03o} ({oct_label:<3}) {sdi:<5} 0x{data:05X} ({data:<7}) {ssm:<5} {p_str:<5} 0x{raw_word:08X}"
                )
    except KeyboardInterrupt:
        print("\n[*] 监听已由用户中止。")
    finally:
        ser.close()


def main() -> None:
    parser = argparse.ArgumentParser(description="Pico-429 ARINC 429 交互与调试工具")
    parser.add_argument("--port", "-p", help="串口名称 (例如 COM3 或 /dev/ttyACM0，未提供时自动搜索)")

    subparsers = parser.add_subparsers(dest="command", required=True)

    # ping
    subparsers.add_parser("ping", help="连通性测试 (发送 PING 期待 PONG)")

    # status
    p_status = subparsers.add_parser("status", help="查询板卡在线状态与通道配置")
    p_status.add_argument("--channel", "-c", type=int, help="指定查询的通道号 (0~1)")

    # send
    p_send = subparsers.add_parser("send", help="向 ARINC 429 发送单字")
    p_send.add_argument("--channel", "-c", type=int, default=0, help="发送通道号 (默认 0)")
    p_send.add_argument("--label", "-l", help="八进制 Label (例如 310)")
    p_send.add_argument("--data", "-d", default="0", help="数据载荷 (十进制或十六进制 0x1234)")
    p_send.add_argument("--sdi", type=int, default=0, choices=[0, 1, 2, 3], help="源/目标标识符 (0~3)")
    p_send.add_argument("--ssm", type=int, default=3, choices=[0, 1, 2, 3], help="符号/状态矩阵 (0~3)")
    p_send.add_argument("--raw", help="直接发送 32 位原始十六进制字 (例如 0x12345678)")

    # listen
    subparsers.add_parser("listen", help="监听并流式解码接收到的 ARINC 429 数据")

    # bit
    p_bit = subparsers.add_parser("bit", help="硬件自收自发 BIT (内建自测试) 环回测试")
    p_bit.add_argument("--tx", type=int, default=0, help="发送通道号 (默认 0)")
    p_bit.add_argument("--rx", type=int, default=1, help="接收通道号 (默认 1)")

    args = parser.parse_args()

    if args.command == "ping":
        cmd_ping(args)
    elif args.command == "status":
        cmd_status(args)
    elif args.command == "send":
        cmd_send(args)
    elif args.command == "listen":
        cmd_listen(args)
    elif args.command == "bit":
        cmd_bit(args)


if __name__ == "__main__":
    main()
