"""ARINC 429 纯算法编解码与向量单元测试"""

from __future__ import annotations

import unittest

from tools.arinc429_codec import (
    ArincWord,
    BinaryFrame,
    FrameFlags,
    FrameType,
    ParityMode,
    crc16_ccitt,
    reverse_byte_bits,
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


if __name__ == "__main__":
    unittest.main()
