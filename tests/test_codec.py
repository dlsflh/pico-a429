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
    format_ack_status,
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
        self.assertIn("成功", format_ack_status(AckStatus.OK))
        self.assertIn("Flash操作失败/槽位为空", format_ack_status(AckStatus.ERR_FLASH))
        self.assertIn("0x04", format_ack_status(AckStatus.ERR_FLASH))
        self.assertIn("未知状态码", format_ack_status(0xEE))

    def test_send_word_and_await_loopback_stream_demux(self) -> None:
        """验证 send_word_and_await_loopback 能在乱序与粘包流中精准解复用并保留跨帧残余字节。"""
        from tools.pico429.client import Pico429Client
        from tools.pico429.transport import Transport

        class MockTransport(Transport):
            def __init__(self) -> None:
                self.written = bytearray()
                self.resp = bytearray()

            def send(self, data: bytes) -> None:
                self.written.extend(data)
                req_frame = BinaryFrame.decode(bytes(self.written[-16:]))
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
                self.resp.extend(rx.encode() + ack.encode() + b"\xa4\x29\x01\x00\x02")

            def recv(self, max_bytes: int = 64) -> bytes:
                chunk = bytes(self.resp[:max_bytes])
                del self.resp[:max_bytes]
                return chunk

            def in_waiting(self) -> int:
                return len(self.resp)

            def reset_input_buffer(self) -> None:
                self.resp.clear()

            def close(self) -> None:
                pass

        client = Pico429Client(MockTransport())
        ack_frame, rx_frame = client.send_word_and_await_loopback(
            tx_ch=0,
            rx_ch=1,
            raw_word=0x12345678,
            timeout=1.0,
        )

        self.assertIsNotNone(ack_frame)
        self.assertIsNotNone(rx_frame)
        assert ack_frame is not None and rx_frame is not None
        self.assertEqual(ack_frame.payload_0, 0x12345678)
        self.assertEqual(rx_frame.raw_word, 0x12345678)
        self.assertEqual(bytes(client.stream_buf), b"\xa4\x29\x01\x00\x02")

    def test_pico429_client_loopback(self) -> None:
        """验证 Pico429Client 在执行 send_word_and_await_loopback 时必须将 raw_word 写入 payload_0。"""
        from unittest.mock import MagicMock

        from tools.pico429.client import Pico429Client

        mock_transport = MagicMock()
        written = []

        def fake_send(data: bytes) -> None:
            written.append(data)

        # 模拟设备立即回送 ACK + ARINC_DATA
        ack_frame = BinaryFrame.new_cmd_ack(channel=0, status=0, seq_num=1)
        rx_frame = BinaryFrame(
            frame_type=FrameType.ARINC_DATA,
            channel=1,
            flags=0,
            seq_num=1,
            payload_0=100,
            payload_1=0xCAFEBABE,
        )
        in_stream = ack_frame.encode() + rx_frame.encode()

        mock_transport.send.side_effect = fake_send
        mock_transport.recv.return_value = in_stream

        client = Pico429Client(mock_transport)
        _ack, rx = client.send_word_and_await_loopback(
            tx_ch=0, rx_ch=1, raw_word=0xCAFEBABE
        )

        self.assertEqual(len(written), 1)
        sent_frame = BinaryFrame.decode(written[0])
        # 核心断言：TX_RAW_WORD 的 raw_word 必须在 payload_0 中！
        self.assertEqual(sent_frame.payload_0, 0xCAFEBABE)
        self.assertEqual(sent_frame.channel, 0)
        self.assertEqual(rx.payload_1, 0xCAFEBABE)

    def test_bit_engine_eight_vectors_pass(self) -> None:
        """验证 BitEngine 能完整闭环运行 8 组基准向量并成功匹配。"""
        from unittest.mock import MagicMock

        from tools.pico429.bit_engine import BitEngine
        from tools.pico429.client import Pico429Client

        mock_transport = MagicMock()

        mock_channels = [(False, False, False)] * 12
        mock_channels[0] = (True, False, True)  # CH0: TX High
        mock_channels[1] = (True, True, True)  # CH1: RX High
        p0, p1 = pack_12_channels(mock_channels)

        def fake_send(data: bytes) -> None:
            f = BinaryFrame.decode(data)
            if f.opcode == Opcode.QUERY_STATUS:
                ack = BinaryFrame.new_cmd_ack(
                    channel=1,
                    status=AckStatus.OK,
                    seq_num=f.seq_num,
                    payload_0=p0,
                    payload_1=p1,
                )
                mock_transport.recv.return_value = ack.encode()
            elif f.opcode == Opcode.TX_RAW_WORD:
                ack = BinaryFrame.new_cmd_ack(
                    channel=f.channel,
                    status=AckStatus.OK,
                    seq_num=f.seq_num,
                    payload_0=f.payload_0,
                )
                rx = BinaryFrame(
                    frame_type=FrameType.ARINC_DATA,
                    channel=1,
                    flags=0,
                    seq_num=f.seq_num,
                    payload_0=100,
                    payload_1=f.payload_0,
                )
                mock_transport.recv.return_value = ack.encode() + rx.encode()
            else:
                ack = BinaryFrame.new_cmd_ack(
                    channel=f.channel, status=AckStatus.OK, seq_num=f.seq_num
                )
                mock_transport.recv.return_value = ack.encode()

        mock_transport.send.side_effect = fake_send
        mock_transport.recv.return_value = b""

        client = Pico429Client(mock_transport)
        report = BitEngine.run_bit(client, tx_ch=0, rx_ch=1)
        self.assertTrue(report.passed)
        self.assertEqual(len(report.vectors), 8)
        for v in report.vectors:
            self.assertTrue(v.matched)

    def test_bit_engine_rejects_in_config_mode(self) -> None:
        """验证处于 CONFIG 静默模式时，BitEngine 坚决拒绝执行自测试。"""
        from unittest.mock import MagicMock

        from tools.pico429.bit_engine import BitEngine
        from tools.pico429.client import Pico429Client

        mock_transport = MagicMock()
        # channel=0 表示 CONFIG 模式
        ack = BinaryFrame.new_cmd_ack(
            channel=0, status=AckStatus.OK, seq_num=1, payload_0=0, payload_1=0
        )
        mock_transport.recv.return_value = ack.encode()

        client = Pico429Client(mock_transport)
        report = BitEngine.run_bit(client, tx_ch=0, rx_ch=1)
        self.assertFalse(report.passed)
        self.assertIsNotNone(report.error_message)
        self.assertIn("CONFIG 静默维护模式", str(report.error_message))

    def test_cli_parser_config_hierarchy(self) -> None:
        """验证重构后的动词子命令式 CLI 解析器能正确解析 config 及其二级子命令。"""
        from tools.pico429.cli.parser import create_parser

        parser = create_parser()

        # 1. config channel
        args = parser.parse_args(
            [
                "config",
                "channel",
                "0",
                "--dir",
                "tx",
                "--speed",
                "100k",
                "--run",
            ]
        )
        self.assertEqual(args.command, "config")
        self.assertEqual(args.config_subcommand, "channel")
        self.assertEqual(args.channel, 0)
        self.assertEqual(args.dir, "tx")
        self.assertTrue(args.run)

        # 2. config channel with filter
        args = parser.parse_args(
            [
                "config",
                "channel",
                "1",
                "--filter",
                "whitelist",
                "--labels",
                "310,203",
            ]
        )
        self.assertEqual(args.filter, "whitelist")
        self.assertEqual(args.labels, "310,203")

        # 3. config mode
        args = parser.parse_args(["config", "mode", "run"])
        self.assertEqual(args.target_mode, "run")

        # 4. status (顶层独立状态命令)
        args = parser.parse_args(["status", "--channel", "0"])
        self.assertEqual(args.command, "status")
        self.assertEqual(args.channel, 0)

        # 5. listen (专职监听器)
        args = parser.parse_args(["listen", "-c", "1"])
        self.assertEqual(args.command, "listen")
        self.assertEqual(args.channel, 1)

        # 6. flash apply (带 --slot 固化)
        args = parser.parse_args(
            ["flash", "apply", "profiles/loopback_ch0_ch1.toml", "--slot", "0"]
        )
        self.assertEqual(args.command, "flash")
        self.assertEqual(args.flash_action, "apply")
        self.assertEqual(args.file, "profiles/loopback_ch0_ch1.toml")
        self.assertEqual(args.slot, 0)

        # 7. flash clear --all
        args = parser.parse_args(["flash", "clear", "--all"])
        self.assertEqual(args.flash_action, "clear")
        self.assertTrue(args.all)

        # 8. bit with --profile
        args = parser.parse_args(["bit", "--profile", "profiles/loopback_ch0_ch1.toml"])
        self.assertEqual(args.command, "bit")
        self.assertEqual(args.profile, "profiles/loopback_ch0_ch1.toml")


if __name__ == "__main__":
    unittest.main()
