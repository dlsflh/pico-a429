"""ARINC 429 协议与编解码核心算法模块 (Zero-I/O Pure Codec)

提供 32 位 ARINC 字解析/组装、八进制 Label 倒序转换、标准奇偶校验以及 16 字节
二进制遥测帧 (BinaryFrame) 与 CRC16-CCITT 校验。无第三方依赖，纯标准库实现。
"""

from __future__ import annotations

import struct
from dataclasses import dataclass
from enum import Enum

BINARY_MAGIC = bytes([0x41, 0x29])  # b"A)"
BINARY_FRAME_LEN = 16


class FrameType:
    ARINC_DATA = 0x01
    CMD_ACK = 0x02
    ASYNC_EVENT = 0x03
    CMD_REQ = 0x04


class Opcode:
    PING = 0x01
    SET_CHANNEL_CFG = 0x02
    SET_FILTER = 0x03
    TX_RAW_WORD = 0x04
    SET_OPERATING_MODE = 0x05
    PAUSE_RESUME_ALL = 0x06
    CFGBATCH = 0x07
    QUERY_STATUS = 0x08
    PROFILE_OP = 0x09


class AckStatus:
    OK = 0x00
    ERR_UNKNOWN_CMD = 0x01
    ERR_INVALID_PARAM = 0x02
    ERR_MODE_CONFLICT = 0x03
    ERR_FLASH = 0x04
    ERR_BUFFER_FULL = 0x05


class ProfileSubOp:
    LIST = 0x00
    SAVE = 0x01
    LOAD = 0x02
    BOOT = 0x03
    CLEAR_BOOT = 0x04


ACK_STATUS_DESCRIPTIONS: dict[int, str] = {
    AckStatus.OK: "成功 (OK)",
    AckStatus.ERR_UNKNOWN_CMD: "未知指令 (ERR_UNKNOWN_CMD)",
    AckStatus.ERR_INVALID_PARAM: "无效参数 (ERR_INVALID_PARAM)",
    AckStatus.ERR_MODE_CONFLICT: "模式冲突/通道未就绪 (ERR_MODE_CONFLICT)",
    AckStatus.ERR_FLASH: "Flash操作失败/槽位为空 (ERR_FLASH)",
    AckStatus.ERR_BUFFER_FULL: "硬件缓冲区已满 (ERR_BUFFER_FULL)",
}


def format_ack_status(status: int) -> str:
    """将应答状态码转换为包含语义说明的人类可读字符串。"""
    desc = ACK_STATUS_DESCRIPTIONS.get(status)
    if desc:
        return f"0x{status:02X} [{desc}]"
    return f"0x{status:02X} [未知状态码]"


class FrameFlags:
    PARITY_ERROR = 1 << 0
    SPEED_100K = 1 << 1
    GAP_ERROR = 1 << 2
    IS_TX_ECHO = 1 << 3
    BUS_FAULT = 1 << 4


class ParityMode(Enum):
    ODD = "odd"
    EVEN = "even"
    NONE = "none"


def reverse_byte_bits(val: int) -> int:
    """8 位字节按位反转 (用于工程八进制 Label 与寄存器总线字节转换)。"""
    res = 0
    for i in range(8):
        if (val >> i) & 1:
            res |= 1 << (7 - i)
    return res


def crc16_ccitt(data: bytes) -> int:
    """CRC-16-CCITT (Poly: 0x1021, Init: 0xFFFF)。"""
    crc = 0xFFFF
    for byte in data:
        crc ^= (byte & 0xFF) << 8
        for _ in range(8):
            if crc & 0x8000:
                crc = ((crc << 1) ^ 0x1021) & 0xFFFF
            else:
                crc = (crc << 1) & 0xFFFF
    return crc


def apply_parity(word31: int, mode: ParityMode = ParityMode.ODD) -> int:
    """根据指定的奇偶校验模式为低 31 位字设置第 32 位 (bit 31)。"""
    base = word31 & 0x7FFF_FFFF
    ones = base.bit_count()
    if mode == ParityMode.ODD:
        return base | 0x8000_0000 if (ones & 1) == 0 else base
    elif mode == ParityMode.EVEN:
        return base | 0x8000_0000 if (ones & 1) == 1 else base
    return word31 & 0xFFFF_FFFF


def check_parity(word32: int, mode: ParityMode = ParityMode.ODD) -> bool:
    """检查 32 位 ARINC 字是否满足指定奇偶校验。"""
    ones = (word32 & 0xFFFF_FFFF).bit_count()
    if mode == ParityMode.ODD:
        return (ones & 1) == 1
    elif mode == ParityMode.EVEN:
        return (ones & 1) == 0
    return True


@dataclass(frozen=True)
class ArincWord:
    """ARINC 429 32 位字模型。"""

    raw: int

    @classmethod
    def from_fields(
        cls,
        label_octal: int,
        sdi: int,
        data: int,
        ssm: int,
        parity_mode: ParityMode = ParityMode.ODD,
    ) -> ArincWord:
        """从工程八进制 Label 与结构化字段组装 ARINC 字。"""
        if not 0 <= label_octal <= 0o377:
            raise ValueError(f"Label 八进制值超出范围 (0~0o377): {oct(label_octal)}")
        if not 0 <= sdi <= 3:
            raise ValueError(f"SDI 超出 2 位范围 (0~3): {sdi}")
        if not 0 <= data <= 0x7FFFF:
            raise ValueError(f"Data 超出 19 位范围 (0~0x7FFFF): 0x{data:X}")
        if not 0 <= ssm <= 3:
            raise ValueError(f"SSM 超出 2 位范围 (0~3): {ssm}")

        bus_label = reverse_byte_bits(label_octal)
        raw_31 = (
            (bus_label & 0xFF)
            | ((sdi & 0x03) << 8)
            | ((data & 0x7FFFF) << 10)
            | ((ssm & 0x03) << 29)
        )
        return cls(raw=apply_parity(raw_31, parity_mode))

    @property
    def label_octal(self) -> int:
        """还原为工程八进制 Label 数值 (例如 0o310)。"""
        bus_byte = self.raw & 0xFF
        return reverse_byte_bits(bus_byte)

    @property
    def label_str(self) -> str:
        """格式化为 3 位八进制字符串 (例如 '310')。"""
        return f"{self.label_octal:03o}"

    @property
    def sdi(self) -> int:
        """2 位源/目的标识符 (Bits 9~10)。"""
        return (self.raw >> 8) & 0x03

    @property
    def data(self) -> int:
        """19 位有效数据载荷 (Bits 11~29)。"""
        return (self.raw >> 10) & 0x7FFFF

    @property
    def ssm(self) -> int:
        """2 位符号/状态矩阵 (Bits 30~31)。"""
        return (self.raw >> 29) & 0x03

    @property
    def parity_bit(self) -> int:
        """第 32 位奇偶校验位。"""
        return (self.raw >> 31) & 0x01

    @property
    def is_parity_valid(self) -> bool:
        """验证标准奇校验。"""
        return check_parity(self.raw, ParityMode.ODD)


@dataclass(frozen=True)
class BinaryFrame:
    """16 字节定长二进制遥测帧。"""

    frame_type: int
    channel: int
    flags: int
    seq_num: int
    timestamp_us: int
    raw_word: int

    def encode(self) -> bytes:
        """打包为 16 字节二进制报文。"""
        type_and_ch = ((self.frame_type & 0x0F) << 4) | (self.channel & 0x0F)
        payload_12 = struct.pack(
            "<BBHII",
            type_and_ch,
            self.flags & 0xFF,
            self.seq_num & 0xFFFF,
            self.timestamp_us & 0xFFFFFFFF,
            self.raw_word & 0xFFFFFFFF,
        )
        crc = crc16_ccitt(payload_12)
        return BINARY_MAGIC + payload_12 + struct.pack("<H", crc)

    @classmethod
    def decode(cls, data: bytes) -> BinaryFrame:
        """从 16 字节切片反序列化并校验 CRC-16。"""
        if len(data) < BINARY_FRAME_LEN:
            raise ValueError(
                f"帧长度不足: 期望 {BINARY_FRAME_LEN} 字节，实际 {len(data)}"
            )
        if data[:2] != BINARY_MAGIC:
            raise ValueError(f"无效 Magic: {data[:2]!r}")

        expected_crc = struct.unpack("<H", data[14:16])[0]
        actual_crc = crc16_ccitt(data[2:14])
        if expected_crc != actual_crc:
            raise ValueError(
                f"CRC 校验失败: 期望 0x{expected_crc:04X}, 实际 0x{actual_crc:04X}"
            )

        type_and_ch, flags, seq_num, ts_us, raw_word = struct.unpack(
            "<BBHII", data[2:14]
        )
        return cls(
            frame_type=(type_and_ch >> 4) & 0x0F,
            channel=type_and_ch & 0x0F,
            flags=flags,
            seq_num=seq_num,
            timestamp_us=ts_us,
            raw_word=raw_word,
        )

    @classmethod
    def new_cmd_req(
        cls,
        channel: int,
        opcode: int,
        seq_num: int,
        payload_0: int = 0,
        payload_1: int = 0,
    ) -> BinaryFrame:
        """组装控制命令请求帧 (CMD_REQ)。"""
        return cls(
            frame_type=FrameType.CMD_REQ,
            channel=channel & 0x0F,
            flags=opcode & 0xFF,
            seq_num=seq_num & 0xFFFF,
            timestamp_us=payload_0 & 0xFFFFFFFF,
            raw_word=payload_1 & 0xFFFFFFFF,
        )

    @classmethod
    def new_cmd_ack(
        cls,
        channel: int,
        status: int,
        seq_num: int,
        payload_0: int = 0,
        payload_1: int = 0,
    ) -> BinaryFrame:
        """组装控制命令应答帧 (CMD_ACK)。"""
        return cls(
            frame_type=FrameType.CMD_ACK,
            channel=channel & 0x0F,
            flags=status & 0xFF,
            seq_num=seq_num & 0xFFFF,
            timestamp_us=payload_0 & 0xFFFFFFFF,
            raw_word=payload_1 & 0xFFFFFFFF,
        )

    @property
    def opcode(self) -> int:
        """针对 CMD_REQ 获取操作码。"""
        return self.flags

    @property
    def ack_status(self) -> int:
        """针对 CMD_ACK 获取应答状态。"""
        return self.flags


def pack_channel_nibble(enable: bool, is_rx: bool, high_speed: bool) -> int:
    """单个通道配置打包为 4 位 nibble。"""
    if not enable:
        return 0
    val = 1
    if is_rx:
        val |= 2
    if high_speed:
        val |= 4
    return val


def unpack_channel_nibble(nibble: int) -> tuple[bool, bool, bool]:
    """解包单个通道 4 位 nibble 为 (enable, is_rx, high_speed)。"""
    enable = bool(nibble & 0x01)
    is_rx = bool(nibble & 0x02)
    high_speed = bool(nibble & 0x04)
    return (enable, is_rx, high_speed)


def pack_12_channels(channels: list[tuple[bool, bool, bool]]) -> tuple[int, int]:
    """12 通道配置编码为两个 32 位载荷字 (Payload 0: CH0..7, Payload 1: CH8..11)。"""
    p0 = 0
    p1 = 0
    for ch in range(min(8, len(channels))):
        en, rx, hs = channels[ch]
        p0 |= pack_channel_nibble(en, rx, hs) << (ch * 4)
    for ch in range(8, min(12, len(channels))):
        en, rx, hs = channels[ch]
        p1 |= pack_channel_nibble(en, rx, hs) << ((ch - 8) * 4)
    return p0, p1


def unpack_12_channels(p0: int, p1: int) -> list[tuple[bool, bool, bool]]:
    """解包两个 32 位载荷字为 12 通道配置列表。"""
    res = []
    for ch in range(8):
        nibble = (p0 >> (ch * 4)) & 0x0F
        res.append(unpack_channel_nibble(nibble))
    for ch in range(4):
        nibble = (p1 >> (ch * 4)) & 0x0F
        res.append(unpack_channel_nibble(nibble))
    return res


def find_frame_in_stream(buf: bytes | bytearray) -> tuple[BinaryFrame, int] | None:
    """在字节切片中采用 1 字节增量滑动窗口搜索第一个合法的 16 字节帧。

    返回 (frame, consumed_bytes)，未找到完整合法帧时返回 None。
    """
    if len(buf) < BINARY_FRAME_LEN:
        return None
    for i in range(len(buf) - BINARY_FRAME_LEN + 1):
        if buf[i : i + 2] == BINARY_MAGIC:
            try:
                frame = BinaryFrame.decode(bytes(buf[i : i + BINARY_FRAME_LEN]))
                return frame, i + BINARY_FRAME_LEN
            except ValueError:
                continue
    return None
