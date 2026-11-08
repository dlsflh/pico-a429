"""Pico-429 硬件设备统一客户端 API (Domain & Client Layer)"""

from __future__ import annotations

import collections
import time
from typing import Self

from tools.arinc429_codec import (
    AckStatus,
    BinaryFrame,
    FrameType,
    Opcode,
    ProfileSubOp,
    find_frame_in_stream,
    pack_12_channels,
    pack_channel_nibble,
    unpack_12_channels,
)
from tools.pico429.transport import Transport


class Pico429Client:
    """高内聚、传输无关的 Pico-429 硬件设备客户端 (消除与 CLI 打印的直接耦合)。"""

    def __init__(self, transport: Transport) -> None:
        self.transport = transport
        self.stream_buf = bytearray()
        self.telemetry_queue: collections.deque[BinaryFrame] = collections.deque(
            maxlen=256
        )
        self._seq = 0

    def close(self) -> None:
        self.transport.close()

    def __enter__(self) -> Self:
        return self

    def __exit__(self, *args: object) -> None:
        self.close()

    def get_next_seq(self) -> int:
        self._seq = (self._seq + 1) & 0xFFFF
        return self._seq

    def send_cmd(
        self,
        opcode: int,
        channel: int = 0,
        payload_0: int = 0,
        payload_1: int = 0,
        timeout: float = 1.0,
    ) -> BinaryFrame:
        """下发控制指令并阻塞等待匹配 seq 的 CMD_ACK 回执，自动将并发遥测存入 telemetry_queue。"""
        seq = self.get_next_seq()
        req = BinaryFrame.new_cmd_req(
            channel=channel,
            opcode=opcode,
            seq_num=seq,
            payload_0=payload_0,
            payload_1=payload_1,
        )
        self.transport.send(req.encode())

        t0 = time.perf_counter()
        while time.perf_counter() - t0 < timeout:
            chunk = self.transport.recv(64)
            if chunk:
                self.stream_buf.extend(chunk)

                while True:
                    res = find_frame_in_stream(self.stream_buf)
                    if res is None:
                        break
                    frame, consumed = res
                    del self.stream_buf[:consumed]

                    if frame.frame_type == FrameType.CMD_ACK and frame.seq_num == seq:
                        return frame
                    elif frame.frame_type == FrameType.ARINC_DATA:
                        self.telemetry_queue.append(frame)
            else:
                time.sleep(0.002)

        raise TimeoutError(
            f"指令 (Opcode 0x{opcode:02X}, Seq {seq}) 等待应答超时 ({timeout}s)"
        )

    def send_cmd_multi_ack(
        self,
        opcode: int,
        expected_count: int = 4,
        channel: int = 0,
        payload_0: int = 0,
        payload_1: int = 0,
        timeout: float = 1.0,
    ) -> list[BinaryFrame]:
        """下发控制指令并收集序列 CMD_ACK 回执 (如 PROFILE LIST)。"""
        seq = self.get_next_seq()
        req = BinaryFrame.new_cmd_req(
            channel=channel,
            opcode=opcode,
            seq_num=seq,
            payload_0=payload_0,
            payload_1=payload_1,
        )
        self.transport.send(req.encode())

        t0 = time.perf_counter()
        acks: list[BinaryFrame] = []

        while time.perf_counter() - t0 < timeout:
            chunk = self.transport.recv(64)
            if chunk:
                self.stream_buf.extend(chunk)

                while True:
                    res = find_frame_in_stream(self.stream_buf)
                    if res is None:
                        break
                    frame, consumed = res
                    del self.stream_buf[:consumed]

                    if frame.frame_type == FrameType.CMD_ACK and frame.seq_num == seq:
                        acks.append(frame)
                        if len(acks) >= expected_count:
                            return acks
                    elif frame.frame_type == FrameType.ARINC_DATA:
                        self.telemetry_queue.append(frame)
            else:
                time.sleep(0.002)

        if not acks:
            raise TimeoutError(
                f"指令 (Opcode 0x{opcode:02X}, Seq {seq}) 等待应答序列超时 ({timeout}s)"
            )
        return acks

    def ping(self, timeout: float = 1.0) -> bool:
        ack = self.send_cmd(Opcode.PING, timeout=timeout)
        return ack.ack_status == AckStatus.OK

    def query_status(
        self, timeout: float = 1.0
    ) -> tuple[int, list[tuple[bool, bool, bool]]]:
        """查询运行模式与 12 通道配置。返回 (mode: 0=Config/1=Operational, channels_list)。"""
        ack = self.send_cmd(Opcode.QUERY_STATUS, timeout=timeout)
        if ack.ack_status != AckStatus.OK:
            raise RuntimeError(f"查询状态失败: AckStatus 0x{ack.ack_status:02X}")
        mode = ack.channel
        channels = unpack_12_channels(ack.payload_0, ack.payload_1)
        return mode, channels

    def set_channel_cfg(
        self, channel: int, enable: bool, is_rx: bool, high_speed: bool
    ) -> BinaryFrame:
        nibble = pack_channel_nibble(enable, is_rx, high_speed)
        return self.send_cmd(Opcode.SET_CHANNEL_CFG, channel=channel, payload_0=nibble)

    def set_filter(
        self, channel: int, action_code: int, octal_label: int = 0
    ) -> BinaryFrame:
        p0 = (action_code & 0xFF) | ((octal_label & 0xFF) << 8)
        return self.send_cmd(Opcode.SET_FILTER, channel=channel, payload_0=p0)

    def set_operating_mode(self, mode: int) -> BinaryFrame:
        return self.send_cmd(Opcode.SET_OPERATING_MODE, payload_0=mode)

    def send_word(self, channel: int, raw_word: int) -> BinaryFrame:
        return self.send_cmd(Opcode.TX_RAW_WORD, channel=channel, payload_0=raw_word)

    def send_word_and_await_loopback(
        self,
        tx_ch: int,
        rx_ch: int,
        raw_word: int,
        timeout: float = 1.0,
    ) -> tuple[BinaryFrame, BinaryFrame]:
        """发送 32 位原始字并精准捕获本地硬件回环数据帧。"""
        # 发射前：将当前 stream_buf 中已完整就绪的历史残留帧安全移入 telemetry_queue 暂存，
        # 避免上一个测试向量的旧帧被误认作本次发射的新回环帧，同时保护并发遥测不丢失
        while True:
            res = find_frame_in_stream(self.stream_buf)
            if res is None:
                break
            frame, consumed = res
            del self.stream_buf[:consumed]
            self.telemetry_queue.append(frame)

        seq = self.get_next_seq()
        req = BinaryFrame.new_cmd_req(
            channel=tx_ch,
            opcode=Opcode.TX_RAW_WORD,
            seq_num=seq,
            payload_0=raw_word,
            payload_1=0,
        )
        self.transport.send(req.encode())

        t0 = time.perf_counter()
        ack_frame: BinaryFrame | None = None
        rx_frame: BinaryFrame | None = None

        while time.perf_counter() - t0 < timeout:
            chunk = self.transport.recv(64)
            if chunk:
                self.stream_buf.extend(chunk)

                while True:
                    res = find_frame_in_stream(self.stream_buf)
                    if res is None:
                        break
                    frame, consumed = res
                    del self.stream_buf[:consumed]

                    if (
                        frame.frame_type == FrameType.CMD_ACK
                        and frame.seq_num == seq
                        and ack_frame is None
                    ):
                        ack_frame = frame
                    elif (
                        frame.frame_type == FrameType.ARINC_DATA
                        and frame.channel == rx_ch
                        and rx_frame is None
                    ):
                        rx_frame = frame
                    else:
                        self.telemetry_queue.append(frame)

                    if ack_frame is not None and rx_frame is not None:
                        return ack_frame, rx_frame
                    if ack_frame is not None and ack_frame.ack_status != 0:
                        raise RuntimeError(
                            f"下位机拒绝发射 (Seq {seq}): AckStatus 0x{ack_frame.ack_status:02X}"
                        )
            else:
                time.sleep(0.002)

        if ack_frame is None:
            raise TimeoutError(f"发送数据字 (Seq {seq}) 未收到下位机回执")
        raise TimeoutError(f"接收通道 CH{rx_ch} 未在 {timeout}s 内收到回环数据帧")

    def cfg_batch(self, channels_list: list[tuple[bool, bool, bool]]) -> BinaryFrame:
        p0, p1 = pack_12_channels(channels_list)
        return self.send_cmd(Opcode.CFGBATCH, payload_0=p0, payload_1=p1)

    def profile_list(self) -> list[BinaryFrame]:
        return self.send_cmd_multi_ack(
            Opcode.PROFILE_OP, expected_count=4, payload_0=ProfileSubOp.LIST
        )

    def profile_save(self, slot: int) -> BinaryFrame:
        return self.send_cmd(
            Opcode.PROFILE_OP, channel=slot, payload_0=ProfileSubOp.SAVE
        )

    def profile_load(self, slot: int) -> BinaryFrame:
        return self.send_cmd(
            Opcode.PROFILE_OP, channel=slot, payload_0=ProfileSubOp.LOAD
        )

    def profile_boot(self, slot: int) -> BinaryFrame:
        return self.send_cmd(
            Opcode.PROFILE_OP, channel=slot, payload_0=ProfileSubOp.BOOT
        )

    def profile_clear_boot(self) -> BinaryFrame:
        return self.send_cmd(
            Opcode.PROFILE_OP, channel=0, payload_0=ProfileSubOp.CLEAR_BOOT
        )

    def profile_clear(self, slot: int) -> BinaryFrame:
        return self.send_cmd(
            Opcode.PROFILE_OP, channel=slot, payload_0=ProfileSubOp.CLEAR
        )

    def profile_clear_all(self) -> BinaryFrame:
        return self.send_cmd(
            Opcode.PROFILE_OP, channel=0, payload_0=ProfileSubOp.CLEAR_ALL
        )

    def set_profile_name(self, slot: int, name: str) -> None:
        """分 8 块 (每块 4 字节) 将 32 字节配置方案名称下发至固件暂存区。"""
        raw_bytes = name.encode("utf-8")[:32].ljust(32, b"\x00")
        for chunk_idx in range(8):
            chunk_4 = raw_bytes[chunk_idx * 4 : (chunk_idx + 1) * 4]
            chunk_val = int.from_bytes(chunk_4, "little")
            p0 = ProfileSubOp.SET_NAME_CHUNK | (chunk_idx << 8)
            self.send_cmd(
                Opcode.PROFILE_OP,
                channel=slot,
                payload_0=p0,
                payload_1=chunk_val,
            )

    def get_profile_name(self, slot: int) -> str:
        """从固件分块读取指定槽位的 32 字节方案名称。"""
        name_bytes = bytearray()
        for chunk_idx in range(8):
            p0 = ProfileSubOp.GET_NAME_CHUNK | (chunk_idx << 8)
            try:
                ack = self.send_cmd(Opcode.PROFILE_OP, channel=slot, payload_0=p0)
                if ack.ack_status != AckStatus.OK:
                    return f"Slot_{slot}"
                name_bytes.extend(ack.payload_1.to_bytes(4, "little"))
            except (OSError, RuntimeError, TimeoutError):
                return f"Slot_{slot}"
        cleaned = bytes(name_bytes).split(b"\x00", 1)[0]
        try:
            name_str = cleaned.decode("utf-8").strip()
            return name_str if name_str else f"Slot_{slot}"
        except UnicodeDecodeError:
            return f"Slot_{slot}"

    def profile_save_with_name(self, slot: int, name: str) -> BinaryFrame:
        """先下发名称，再持久化保存配置到指定槽位。"""
        self.set_profile_name(slot, name)
        return self.profile_save(slot)

    def profile_get_channels(
        self, slot: int
    ) -> tuple[bool, list[tuple[bool, bool, bool]]]:
        ack = self.send_cmd(
            Opcode.PROFILE_OP, channel=slot, payload_0=ProfileSubOp.GET_CHANNELS
        )
        if ack.ack_status != AckStatus.OK:
            return False, [(False, False, False)] * 12
        return True, unpack_12_channels(ack.payload_0, ack.payload_1)

    def reboot_to_bootsel(self) -> BinaryFrame:
        return self.send_cmd(Opcode.REBOOT_TO_BOOTSEL)
