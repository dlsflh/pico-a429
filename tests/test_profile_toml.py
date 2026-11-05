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
        self.assertEqual(parsed["channels"][0], {"direction": "TX", "speed": "HIGH", "filter": []})
        self.assertEqual(parsed["channels"][2], {"direction": "RX", "speed": "LOW", "filter": []})
        self.assertEqual(parsed["channels"][6], {"direction": "DISABLED", "speed": "LOW", "filter": []})

        # 验证批量下发指令生成 (未显式指定的通道默认 DISABLED)
        batch_spec = build_batch_spec(parsed["channels"])
        parts = batch_spec.split(",")
        self.assertEqual(len(parts), 12)
        self.assertEqual(parts[0], "0=TX:HIGH")
        self.assertEqual(parts[2], "2=RX:LOW")

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


if __name__ == "__main__":
    unittest.main()
