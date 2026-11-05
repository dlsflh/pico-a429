"""ARINC 429 Pico 2 / Pico 2 W 串口传输与底层通信模块 (Serial Transport)"""

from __future__ import annotations

import sys
import time
import typing

if typing.TYPE_CHECKING:
    import serial  # type: ignore[import-untyped,unresolved-import]  # ty: ignore[unresolved-import]
    import serial.tools.list_ports  # type: ignore[import-untyped,unresolved-import]  # ty: ignore[unresolved-import]
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
        BinaryFrame,
        FrameType,
    )
except ModuleNotFoundError:
    from arinc429_codec import (  # type: ignore[no-redef]
        BINARY_FRAME_LEN,
        BINARY_MAGIC,
        BinaryFrame,
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
                        return (
                            f"OK: Command acknowledged (binary mode, seq {f.seq_num})"
                        )
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
