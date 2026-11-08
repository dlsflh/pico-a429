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
            payload_0=1234567,
            payload_1=0xE048D013,
        )
        encoded = frame.encode()
        self.assertEqual(len(encoded), 16)
        self.assertEqual(encoded[:2], b"A)")

        decoded = BinaryFrame.decode(encoded)
        self.assertEqual(decoded, frame)
        self.assertEqual(decoded.channel, 1)
        self.assertEqual(decoded.seq_num, 42)
        self.assertEqual(decoded.raw_word, 0xE048D013)
        self.assertEqual(decoded.timestamp_us, 1234567)

    def test_binary_frame_crc_corruption(self) -> None:
        frame = BinaryFrame(
            frame_type=FrameType.ARINC_DATA,
            channel=0,
            flags=0,
            seq_num=1,
            payload_0=100,
            payload_1=0x12345678,
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
                    payload_0=1000,
                    payload_1=0x12345678,
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

        transport = MockTransport()
        client = Pico429Client(transport)
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

        # 核心契约验证：发出的 TX_RAW_WORD 必须在 payload_0 中携带 raw_word 且 channel 对齐
        self.assertEqual(len(transport.written), 16)
        sent_frame = BinaryFrame.decode(bytes(transport.written))
        self.assertEqual(sent_frame.opcode, Opcode.TX_RAW_WORD)
        self.assertEqual(sent_frame.channel, 0)
        self.assertEqual(sent_frame.payload_0, 0x12345678)

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

    def test_send_word_and_await_loopback_drains_stale_stream_buf(self) -> None:
        """验证 send_word_and_await_loopback 在发射前自动排空 stream_buf 中残留的历史帧至 telemetry_queue。"""
        from tools.pico429.client import Pico429Client

        class MockTransport:
            def __init__(self, responses: list[bytes]) -> None:
                self.responses = responses
                self.sent: list[bytes] = []

            def send(self, data: bytes) -> None:
                self.sent.append(data)

            def recv(self, max_bytes: int = 64) -> bytes:
                if self.responses:
                    return self.responses.pop(0)
                return b""

            def in_waiting(self) -> int:
                return sum(len(c) for c in self.responses)

            def reset_input_buffer(self) -> None:
                self.responses.clear()

            def close(self) -> None:
                pass

        # 构造上一个测试遗留的旧帧 (CH1, 0xE048D013)
        old_frame = BinaryFrame(
            frame_type=FrameType.ARINC_DATA,
            channel=1,
            flags=FrameFlags.SPEED_100K,
            seq_num=99,
            payload_0=1000,
            payload_1=0xE048D013,
        )

        # 构造本次发射的 ACK 帧与新回环帧 (CH1, 0x60159DC1)
        # 注意：Pico429Client 初始 seq 为 0，第一次 get_next_seq() 返回 1
        ack_frame = BinaryFrame.new_cmd_ack(
            channel=0,
            status=AckStatus.OK,
            seq_num=1,
            payload_0=0x60159DC1,
        )
        new_loopback_frame = BinaryFrame(
            frame_type=FrameType.ARINC_DATA,
            channel=1,
            flags=FrameFlags.SPEED_100K,
            seq_num=100,
            payload_0=2000,
            payload_1=0x60159DC1,
        )

        transport = MockTransport([ack_frame.encode(), new_loopback_frame.encode()])
        client = Pico429Client(transport)

        # 模拟发射前 stream_buf 中滞留了旧帧
        client.stream_buf.extend(old_frame.encode())

        # 调用回环发送并等待捕获
        ret_ack, ret_rx = client.send_word_and_await_loopback(
            tx_ch=0,
            rx_ch=1,
            raw_word=0x60159DC1,
        )

        # 验证：捕获到的回环帧必须是新发射的 0x60159DC1，而不是残留的 0xE048D013！
        self.assertEqual(ret_rx.payload_1, 0x60159DC1)
        self.assertEqual(ret_ack.seq_num, 1)

        # 验证：旧帧并未被物理丢弃，而是被安全移入 telemetry_queue 暂存
        self.assertEqual(len(client.telemetry_queue), 1)
        stashed = client.telemetry_queue.popleft()
        self.assertEqual(stashed.payload_1, 0xE048D013)
        self.assertEqual(stashed.channel, 1)


if __name__ == "__main__":
    unittest.main()
