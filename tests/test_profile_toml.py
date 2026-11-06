"""ARINC 429 12 通道配置方案与 TOML 编解码行为测试"""

from __future__ import annotations

import unittest

from tools.pico429 import (
    build_batch_spec,
    generate_toml_from_cfgdump,
    normalize_speed,
    parse_profile_list_output,
    parse_toml_profile,
)


class TestProfileToml(unittest.TestCase):
    def test_toml_roundtrip_all_channels(self) -> None:
        """验证 12 通道硬件状态导出为 TOML 后能够无损回环反序列化。"""
        cfgdump = (
            "CFGDUMP: 0:TX:HIGH 1:RX:HIGH 2:RX:LOW 3:DISABLED:HIGH 4:TX:LOW "
            "5:RX:HIGH 6:DISABLED:LOW 7:TX:HIGH 8:RX:LOW 9:DISABLED:HIGH "
            "10:TX:HIGH 11:RX:HIGH"
        )
        toml_text = generate_toml_from_cfgdump(cfgdump, profile_name="Bench_12CH")
        self.assertIn('name = "Bench_12CH"', toml_text)

        parsed = parse_toml_profile(toml_text)
        self.assertEqual(parsed["name"], "Bench_12CH")
        self.assertEqual(len(parsed["channels"]), 12)

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

        # 验证批量下发指令生成 (未显式指定的通道默认 DISABLED)
        batch_spec = build_batch_spec(parsed["channels"])
        parts = batch_spec.split(",")
        self.assertEqual(len(parts), 12)
        self.assertEqual(parts[0], "0=TX:HIGH")
        self.assertEqual(parts[2], "2=RX:LOW")

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

    def test_profile_list_parser(self) -> None:
        """验证固件片上 Flash PROFILE LIST 响应解析。"""
        lines = [
            "PROFILES: Boot Slot: Some(2)",
            '[Slot 0] ACTIVE "Bench"',
            "[Slot 1] EMPTY",
            '[Slot 2] ACTIVE "Flight"',
            "[Slot 3] EMPTY",
        ]
        boot_slot, slots = parse_profile_list_output(lines)
        self.assertEqual(boot_slot, 2)
        self.assertEqual(len(slots), 4)
        self.assertEqual(slots[0], {"slot": 0, "status": "ACTIVE", "name": "Bench"})
        self.assertEqual(slots[1]["status"], "EMPTY")

    def test_sample_profiles_validity(self) -> None:
        """验证 profiles/ 目录下所有预置的 TOML 配置方案均可成功解析且结构完备。"""
        import pathlib

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
