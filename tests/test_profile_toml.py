"""ARINC 429 12 通道配置方案与 TOML 编解码行为测试"""

from __future__ import annotations

import pathlib
import unittest

from tools.arinc429_codec import pack_12_channels, unpack_12_channels
from tools.pico429.profile import normalize_speed, parse_toml_profile


class TestProfileToml(unittest.TestCase):
    def test_toml_roundtrip_all_channels(self) -> None:
        """验证 12 通道 TOML 方案解析与底层二进制 pack/unpack 12 通道无损回环。"""
        toml_content = """
        [profile]
        name = "Bench_12CH"
        version = "1.0"

        [channel.0]
        direction = "TX"
        speed = "HIGH"

        [channel.1]
        direction = "RX"
        speed = "HIGH"

        [channel.2]
        direction = "RX"
        speed = "LOW"

        [channel.6]
        direction = "DISABLED"
        speed = "LOW"
        """
        parsed = parse_toml_profile(toml_content)
        self.assertEqual(parsed["name"], "Bench_12CH")
        self.assertEqual(len(parsed["channels"]), 4)

        # 抽样验证核心通道的方向与波特率保持一致
        self.assertEqual(
            parsed["channels"][0],
            {
                "direction": "TX",
                "speed": "HIGH",
                "filter_mode": "none",
                "labels": [],
            },
        )
        self.assertEqual(
            parsed["channels"][2],
            {
                "direction": "RX",
                "speed": "LOW",
                "filter_mode": "none",
                "labels": [],
            },
        )
        self.assertEqual(
            parsed["channels"][6],
            {
                "direction": "DISABLED",
                "speed": "LOW",
                "filter_mode": "none",
                "labels": [],
            },
        )

        # 验证真实生产链路：TOML 转换为 12 通道元组 -> pack_12_channels -> unpack_12_channels
        channels_list = []
        for ch in range(12):
            cfg = parsed["channels"].get(ch, {"direction": "DISABLED", "speed": "HIGH"})
            d = cfg.get("direction", "DISABLED").upper()
            en = d != "DISABLED"
            is_rx = d == "RX"
            s = cfg.get("speed", "HIGH").upper()
            hs = bool(en and ("HIGH" in s or "100" in s))
            channels_list.append((en, is_rx, hs))

        p0, p1 = pack_12_channels(channels_list)
        unpacked = unpack_12_channels(p0, p1)
        self.assertEqual(channels_list, unpacked)
        self.assertEqual(unpacked[0], (True, False, True))  # CH0: Tx, High
        self.assertEqual(unpacked[1], (True, True, True))  # CH1: Rx, High
        self.assertEqual(unpacked[2], (True, True, False))  # CH2: Rx, Low
        self.assertEqual(unpacked[6], (False, False, False))  # CH6: Disabled

    def test_filter_mode_whitelist_and_blacklist(self) -> None:
        """验证新版 filter_mode 与 labels 字段能够正确解析与校验。"""
        toml_whitelist = """
        [profile]
        name = "Whitelist_Test"

        [channel.1]
        direction = "RX"
        speed = "100k"
        filter_mode = "whitelist"
        labels = ["310", "203", "001"]
        """
        parsed = parse_toml_profile(toml_whitelist)
        ch1 = parsed["channels"][1]
        self.assertEqual(ch1["filter_mode"], "whitelist")
        self.assertEqual(ch1["labels"], ["310", "203", "001"])

        toml_blacklist = """
        [profile]
        name = "Blacklist_Test"

        [channel.2]
        direction = "RX"
        speed = "12.5k"
        filter_mode = "blacklist"
        labels = ["377"]
        """
        parsed_bl = parse_toml_profile(toml_blacklist)
        ch2 = parsed_bl["channels"][2]
        self.assertEqual(ch2["filter_mode"], "blacklist")
        self.assertEqual(ch2["labels"], ["377"])

    def test_filter_obsolete_rejection_and_validation(self) -> None:
        """验证废弃旧语法被严格拦截 (Zero Backward-Compatibility Glue) 与参数防呆。"""
        # 1. 废弃的旧版 filter 列表严格拒收，决不静默向下兼容
        toml_obsolete = """
        [channel.0]
        direction = "RX"
        filter = ["ALLOW 310"]
        """
        with self.assertRaises(ValueError) as ctx:
            parse_toml_profile(toml_obsolete)
        self.assertIn("已废弃", str(ctx.exception))

        # 2. 非法 filter_mode
        toml_bad_mode = """
        [channel.0]
        direction = "RX"
        filter_mode = "graylist"
        labels = ["310"]
        """
        with self.assertRaises(ValueError) as ctx:
            parse_toml_profile(toml_bad_mode)
        self.assertIn("无效过滤模式", str(ctx.exception))

        # 3. 非法八进制 Label (包含 8 或 9，或超 377)
        toml_bad_octal = """
        [channel.0]
        direction = "RX"
        filter_mode = "whitelist"
        labels = ["380"]
        """
        with self.assertRaises(ValueError) as ctx:
            parse_toml_profile(toml_bad_octal)
        self.assertIn("无效八进制 Label", str(ctx.exception))

    def test_toml_validation_fail_fast(self) -> None:
        """验证越界通道号、非法工作方向或损毁格式严格拦截。"""
        # 通道超限 (12 > 11)
        with self.assertRaises(ValueError):
            parse_toml_profile('[channel.12]\ndirection = "TX"')

        # 非法方向
        with self.assertRaises(ValueError):
            parse_toml_profile('[channel.0]\ndirection = "INVALID"')

        # 非法速率
        with self.assertRaises(ValueError):
            normalize_speed("50k")

        # 语法损坏
        with self.assertRaises(ValueError):
            parse_toml_profile("[unclosed toml")

    def test_sample_profiles_validity(self) -> None:
        """验证 profiles/ 目录下所有预置的 TOML 配置方案均可成功解析且结构完备。"""
        profiles_dir = pathlib.Path(__file__).parent.parent / "profiles"
        self.assertTrue(profiles_dir.exists())

        for p_file in (
            "bit_loopback.toml",
            "avionics_bench.toml",
            "default_silent.toml",
        ):
            fpath = profiles_dir / p_file
            self.assertTrue(fpath.exists(), f"预置配置文件缺失: {p_file}")
            content = fpath.read_text(encoding="utf-8")
            parsed = parse_toml_profile(content)
            self.assertIn("name", parsed)
            self.assertEqual(len(parsed["channels"]), 12)

        # 针对 bit_loopback.toml 抽样
        bit_cfg = parse_toml_profile(
            (profiles_dir / "bit_loopback.toml").read_text(encoding="utf-8")
        )
        self.assertEqual(bit_cfg["name"], "BIT_Loopback")
        self.assertEqual(bit_cfg["channels"][0]["direction"], "TX")
        self.assertEqual(bit_cfg["channels"][1]["direction"], "RX")
        self.assertEqual(bit_cfg["channels"][2]["direction"], "DISABLED")


if __name__ == "__main__":
    unittest.main()
