"""ARINC 429 纯算法编解码与向量单元测试"""

from __future__ import annotations

import unittest

from tools.arinc429_codec import (
    AckStatus,
    ArincWord,
    BinaryFrame,
    FrameFlags,
    FrameType,
    Opcode,
    ParityMode,
    crc16_ccitt,
    find_frame_in_stream,
    pack_12_channels,
    pack_channel_nibble,
    reverse_byte_bits,
    unpack_12_channels,
    unpack_channel_nibble,
)


class TestArinc429Codec(unittest.TestCase):
    def test_bit_reversal(self) -> None:
        # 八进制 310 (0xC8 = 200, 0b11001000) -> 翻转后为 0x13 (19, 0b00010011)
        self.assertEqual(reverse_byte_bits(0o310), 0x13)
        self.assertEqual(reverse_byte_bits(0x13), 0o310)
        self.assertEqual(reverse_byte_bits(0o000), 0x00)
        self.assertEqual(reverse_byte_bits(0o377), 0xFF)

    def test_official_bit_vectors(self) -> None:
        """验证与下位机固件及 Rust CLI 100% 对齐的 8 组基准向量。"""
        vectors = [
            (0o310, 0, 0x01234, 3, 0xE048D013),
            (0o203, 1, 0x00567, 3, 0x60159DC1),
            (0o001, 2, 0x00001, 3, 0x60000680),
            (0o377, 3, 0x7FFFF, 3, 0x7FFFFFFF),
            (0o156, 0, 0x00000, 0, 0x00000076),
            (0o114, 1, 0x55555, 3, 0xF5555532),
            (0o220, 2, 0x2AAAA, 3, 0xEAAAAA09),
            (0o045, 3, 0x13579, 1, 0xA4D5E7A4),
        ]

        for label, sdi, data, ssm, expected_raw in vectors:
            word = ArincWord.from_fields(label, sdi, data, ssm, ParityMode.ODD)
            self.assertEqual(
                word.raw,
                expected_raw,
                f"Label {oct(label)} 构造失败: 期望 0x{expected_raw:08X}, 实际 0x{word.raw:08X}",
            )
            # 反向提取校验
            self.assertEqual(word.label_octal, label)
            self.assertEqual(word.sdi, sdi)
            self.assertEqual(word.data, data)
            self.assertEqual(word.ssm, ssm)
            self.assertTrue(word.is_parity_valid)

    def test_invalid_parameters_fail_fast(self) -> None:
        with self.assertRaises(ValueError):
            ArincWord.from_fields(0o400, 0, 0, 0)
        with self.assertRaises(ValueError):
            ArincWord.from_fields(0o310, 4, 0, 0)
        with self.assertRaises(ValueError):
            ArincWord.from_fields(0o310, 0, 0x80000, 0)
        with self.assertRaises(ValueError):
            ArincWord.from_fields(0o310, 0, 0, 4)

    def test_binary_frame_roundtrip(self) -> None:
        frame = BinaryFrame(
            frame_type=FrameType.ARINC_DATA,
            channel=1,
            flags=FrameFlags.SPEED_100K,
            seq_num=42,
            timestamp_us=1234567,
            raw_word=0xE048D013,
        )
        encoded = frame.encode()
        self.assertEqual(len(encoded), 16)
        self.assertEqual(encoded[:2], b"A)")

        decoded = BinaryFrame.decode(encoded)
        self.assertEqual(decoded, frame)
        self.assertEqual(decoded.channel, 1)
        self.assertEqual(decoded.seq_num, 42)
        self.assertEqual(decoded.raw_word, 0xE048D013)

    def test_binary_frame_crc_corruption(self) -> None:
        frame = BinaryFrame(
            frame_type=FrameType.ARINC_DATA,
            channel=0,
            flags=0,
            seq_num=1,
            timestamp_us=100,
            raw_word=0x12345678,
        )
        encoded = bytearray(frame.encode())
        # 篡改数据负载
        encoded[5] ^= 0xFF
        with self.assertRaises(ValueError):
            BinaryFrame.decode(bytes(encoded))

    def test_crc16_ccitt(self) -> None:
        test_data = b"123456789"
        # CCITT-FALSE (init 0xFFFF, poly 0x1021) 针对 "123456789" 的标准校验值为 0x29B1
        self.assertEqual(crc16_ccitt(test_data), 0x29B1)

    def test_cmd_req_and_ack_roundtrip(self) -> None:
        req = BinaryFrame.new_cmd_req(
            channel=2,
            opcode=Opcode.SET_CHANNEL_CFG,
            seq_num=101,
            payload_0=0x5,
            payload_1=0x0,
        )
        encoded = req.encode()
        self.assertEqual(len(encoded), 16)
        decoded = BinaryFrame.decode(encoded)
        self.assertEqual(decoded.frame_type, FrameType.CMD_REQ)
        self.assertEqual(decoded.channel, 2)
        self.assertEqual(decoded.opcode, Opcode.SET_CHANNEL_CFG)
        self.assertEqual(decoded.seq_num, 101)
        self.assertEqual(decoded.timestamp_us, 0x5)

        ack = BinaryFrame.new_cmd_ack(
            channel=2,
            status=AckStatus.OK,
            seq_num=101,
            payload_0=0x5,
            payload_1=0x0,
        )
        encoded_ack = ack.encode()
        decoded_ack = BinaryFrame.decode(encoded_ack)
        self.assertEqual(decoded_ack.frame_type, FrameType.CMD_ACK)
        self.assertEqual(decoded_ack.ack_status, AckStatus.OK)
        self.assertEqual(decoded_ack.seq_num, 101)

    def test_channel_nibble_and_12ch_packing(self) -> None:
        nib = pack_channel_nibble(enable=True, is_rx=True, high_speed=True)
        en, is_rx, hs = unpack_channel_nibble(nib)
        self.assertTrue(en)
        self.assertTrue(is_rx)
        self.assertTrue(hs)

        channels = [(False, False, False)] * 12
        channels[0] = (True, False, True)  # CH0: Tx, 100k
        channels[1] = (True, True, False)  # CH1: Rx, 12.5k
        channels[7] = (True, True, True)  # CH7: Rx, 100k
        channels[8] = (True, False, False)  # CH8: Tx, 12.5k
        channels[11] = (True, True, True)  # CH11: Rx, 100k

        p0, p1 = pack_12_channels(channels)
        unpacked = unpack_12_channels(p0, p1)
        self.assertEqual(channels, unpacked)

    def test_sliding_window_find_frame(self) -> None:
        frame = BinaryFrame.new_cmd_req(
            channel=0, opcode=Opcode.PING, seq_num=1, payload_0=0xAABBCCDD
        )
        frame_bytes = frame.encode()

        # 模拟包含 3 字节噪声前缀的串口流
        stream = b"\xff\x41\x00" + frame_bytes + b"\x12\x34"
        res = find_frame_in_stream(stream)
        self.assertIsNotNone(res)
        assert res is not None
        found_frame, consumed = res
        self.assertEqual(found_frame, frame)
        self.assertEqual(consumed, 3 + 16)

    def test_format_ack_status(self) -> None:
        """验证应答状态码人类可读格式化函数。"""
        from tools.arinc429_codec import AckStatus, format_ack_status

        self.assertIn("成功", format_ack_status(AckStatus.OK))
        self.assertIn("Flash操作失败/槽位为空", format_ack_status(AckStatus.ERR_FLASH))
        self.assertIn("0x04", format_ack_status(AckStatus.ERR_FLASH))
        self.assertIn("未知状态码", format_ack_status(0xEE))

    def test_send_word_and_await_loopback_stream_demux(self) -> None:
        """验证 send_word_and_await_loopback 能在乱序与粘包流中精准解复用并保留跨帧残余字节。"""
        from unittest.mock import MagicMock

        from tools.pico429_transport import send_word_and_await_loopback

        mock_ser = MagicMock()
        written_bytes = bytearray()

        def fake_write(data: bytes) -> int:
            written_bytes.extend(data)
            return len(data)

        mock_ser.write.side_effect = fake_write

        # 模拟设备端回传：同时产生 CMD_ACK 与 ARINC_DATA 帧，并附带下一个帧的前 5 字节
        def fake_read(_n: int) -> bytes:
            req_frame = BinaryFrame.decode(bytes(written_bytes[-16:]))
            seq = req_frame.seq_num
            ack = BinaryFrame.new_cmd_ack(
                channel=0, status=0, seq_num=seq, payload_0=0x12345678
            )
            rx = BinaryFrame(
                frame_type=FrameType.ARINC_DATA,
                channel=1,
                flags=0,
                seq_num=42,
                timestamp_us=1000,
                raw_word=0x12345678,
            )
            # 拼接: ARINC_DATA (16 字节) + CMD_ACK (16 字节) + 下一帧的魔数残字节 (5 字节)
            chunk = rx.encode() + ack.encode() + b"\xa4\x29\x01\x00\x02"
            mock_ser.in_waiting = 0
            return chunk

        mock_ser.in_waiting = 37
        mock_ser.read.side_effect = fake_read

        stream_buf = bytearray()
        ack_frame, rx_frame = send_word_and_await_loopback(
            mock_ser,
            tx_ch=0,
            rx_ch=1,
            raw_word=0x12345678,
            stream_buf=stream_buf,
            timeout=1.0,
        )

        self.assertIsNotNone(ack_frame)
        self.assertIsNotNone(rx_frame)
        assert ack_frame is not None and rx_frame is not None
        self.assertEqual(ack_frame.ack_status, 0)
        self.assertEqual(rx_frame.raw_word, 0x12345678)
        self.assertEqual(rx_frame.channel, 1)

        # 验证跨调用残字节保留在 stream_buf 中，完全没有被丢弃
        self.assertEqual(bytes(stream_buf), b"\xa4\x29\x01\x00\x02")


if __name__ == "__main__":
    unittest.main()
