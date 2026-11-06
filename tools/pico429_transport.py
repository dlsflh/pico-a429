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
        find_frame_in_stream,
    )
except ModuleNotFoundError:
    from arinc429_codec import (  # type: ignore[no-redef]  # ty: ignore[unresolved-import]
        BINARY_FRAME_LEN,
        BINARY_MAGIC,
        BinaryFrame,
        FrameType,
        find_frame_in_stream,
    )

_HOST_SEQ = 0


def get_next_seq() -> int:
    """获取单调自增的主机端 16 位请求序号。"""
    global _HOST_SEQ
    _HOST_SEQ = (_HOST_SEQ + 1) & 0xFFFF
    return _HOST_SEQ


def send_binary_cmd(
    ser: serial.Serial,
    opcode: int,
    channel: int = 0,
    payload_0: int = 0,
    payload_1: int = 0,
    timeout: float = 1.0,
) -> BinaryFrame:
    """下发二进制控制指令并以滑动窗口流解复用等待匹配的 CMD_ACK。

    当总线上存在高波特率 ARINC 遥测下行流量时，自动跳过 ARINC_DATA 帧并将其解复用，
    精准依据 seq_num 匹配对应的控制应答，杜绝单 CDC IN 端点读冲突。
    """
    seq = get_next_seq()
    req = BinaryFrame.new_cmd_req(
        channel=channel,
        opcode=opcode,
        seq_num=seq,
        payload_0=payload_0,
        payload_1=payload_1,
    )
    ser.write(req.encode())
    ser.flush()

    t0 = time.perf_counter()
    stream_buf = bytearray()

    while time.perf_counter() - t0 < timeout:
        n = ser.in_waiting
        if n > 0:
            chunk = ser.read(n)
            stream_buf.extend(chunk)

            while True:
                res = find_frame_in_stream(stream_buf)
                if res is None:
                    break
                frame, consumed = res
                del stream_buf[:consumed]

                if frame.frame_type == FrameType.CMD_ACK and frame.seq_num == seq:
                    return frame
        else:
            time.sleep(0.002)

    raise TimeoutError(
        f"二进制指令 (Opcode 0x{opcode:02X}, Seq {seq}) 等待应答超时 ({timeout}s)"
    )


def send_binary_cmd_multi_ack(
    ser: serial.Serial,
    opcode: int,
    expected_count: int = 4,
    channel: int = 0,
    payload_0: int = 0,
    payload_1: int = 0,
    timeout: float = 1.0,
) -> list[BinaryFrame]:
    """下发二进制控制指令并收集指定数量的 CMD_ACK 序列 (针对 PROFILE LIST)。"""
    seq = get_next_seq()
    req = BinaryFrame.new_cmd_req(
        channel=channel,
        opcode=opcode,
        seq_num=seq,
        payload_0=payload_0,
        payload_1=payload_1,
    )
    ser.write(req.encode())
    ser.flush()

    t0 = time.perf_counter()
    stream_buf = bytearray()
    acks: list[BinaryFrame] = []

    while time.perf_counter() - t0 < timeout:
        n = ser.in_waiting
        if n > 0:
            chunk = ser.read(n)
            stream_buf.extend(chunk)

            while True:
                res = find_frame_in_stream(stream_buf)
                if res is None:
                    break
                frame, consumed = res
                del stream_buf[:consumed]

                if frame.frame_type == FrameType.CMD_ACK and frame.seq_num == seq:
                    acks.append(frame)
                    if len(acks) >= expected_count:
                        return acks
        else:
            time.sleep(0.002)

    if not acks:
        raise TimeoutError(
            f"二进制指令 (Opcode 0x{opcode:02X}, Seq {seq}) 等待应答序列超时 ({timeout}s)"
        )
    return acks


def send_word_and_await_loopback(
    ser: serial.Serial,
    tx_ch: int,
    rx_ch: int,
    raw_word: int,
    stream_buf: bytearray,
    timeout: float = 1.0,
) -> tuple[BinaryFrame | None, BinaryFrame | None]:
    """向指定 TX 通道发送 32 位 ARINC 字，并在同一条持久流中匹配 CMD_ACK 与 ARINC_DATA 环回帧。

    返回 (ack_frame, rx_frame)。若在超时内未能完全集齐，则缺失项为 None。
    流缓冲区 stream_buf 跨调用持久保留，杜绝任何未决残字节丢失或截断。
    """
    seq = get_next_seq()
    # Opcode.TX_RAW_WORD = 0x04
    req = BinaryFrame.new_cmd_req(
        channel=tx_ch,
        opcode=0x04,
        seq_num=seq,
        payload_0=raw_word,
    )
    ser.write(req.encode())
    ser.flush()

    t0 = time.perf_counter()
    ack_frame: BinaryFrame | None = None
    rx_frame: BinaryFrame | None = None

    while time.perf_counter() - t0 < timeout:
        # 先排空当前 stream_buf 中已接收但未匹配的帧
        while True:
            res = find_frame_in_stream(stream_buf)
            if res is None:
                break
            frame, consumed = res
            del stream_buf[:consumed]

            if frame.frame_type == FrameType.CMD_ACK and frame.seq_num == seq:
                ack_frame = frame
            elif frame.frame_type == FrameType.ARINC_DATA and frame.channel == rx_ch:
                rx_frame = frame

        if ack_frame is not None and rx_frame is not None:
            break

        n = ser.in_waiting
        if n > 0:
            stream_buf.extend(ser.read(n))
        else:
            time.sleep(0.001)

    return ack_frame, rx_frame


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
