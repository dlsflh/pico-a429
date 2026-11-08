"""ARINC 429 通信底层传输抽象与驱动实现 (Transport Layer - OCP)"""

from __future__ import annotations

import contextlib
import sys
import typing
from typing import Protocol, runtime_checkable

if typing.TYPE_CHECKING:
    import serial  # type: ignore[import-untyped,unresolved-import]  # ty: ignore[unresolved-import]
    import serial.tools.list_ports  # type: ignore[import-untyped,unresolved-import]  # ty: ignore[unresolved-import]
else:
    try:
        import serial
        import serial.tools.list_ports
    except ImportError:
        serial = None


@runtime_checkable
class Transport(Protocol):
    """通信底层统一传输协议契约 (满足开闭原则 OCP)。"""

    def send(self, data: bytes) -> None:
        """发送原始二进制字节流。"""
        ...

    def recv(self, max_bytes: int = 64) -> bytes:
        """非阻塞或带超时读取字节流。"""
        ...

    def in_waiting(self) -> int:
        """查询底层输入缓冲区可读字节数。"""
        ...

    def reset_input_buffer(self) -> None:
        """清空输入缓冲区滞留残留数据。"""
        ...

    def close(self) -> None:
        """释放底层连接资源。"""
        ...


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


class SerialTransport:
    """USB CDC ACM 虚拟串口传输实现。"""

    def __init__(self, ser: serial.Serial) -> None:
        self.ser = ser

    @classmethod
    def open(cls, port_name: str, timeout: float = 0.5) -> SerialTransport:
        if serial is None:
            print("错误: 未安装 pyserial。", file=sys.stderr)
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
            return cls(ser)
        except serial.SerialException as e:
            print(f"错误: 无法打开串口 {port_name}: {e}", file=sys.stderr)
            sys.exit(1)

    def send(self, data: bytes) -> None:
        self.ser.write(data)
        self.ser.flush()

    def recv(self, max_bytes: int = 64) -> bytes:
        n = self.ser.in_waiting
        if n > 0:
            return self.ser.read(min(n, max_bytes))
        return b""

    def in_waiting(self) -> int:
        return self.ser.in_waiting

    def reset_input_buffer(self) -> None:
        self.ser.reset_input_buffer()

    def close(self) -> None:
        with contextlib.suppress(Exception):
            self.ser.close()
