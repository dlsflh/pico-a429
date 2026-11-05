"""ARINC 429 12 通道配置方案与 TOML 编解码单元测试"""

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
    def test_normalize_speed(self) -> None:
        """测试波特率标准化转换 (支持规范命名 high/low 与兼容别名)。"""
        # 高速 (100 kbps) 别名
        for s in ("high", "HIGH", "High", "100k", "100K", "100", "100kbps"):
            self.assertEqual(normalize_speed(s), "HIGH")

        # 低速 (12.5 kbps) 别名
        for s in ("low", "LOW", "Low", "12.5k", "12K5", "12k5", "12.5", "12.5kbps"):
            self.assertEqual(normalize_speed(s), "LOW")

        # 非法速率严格报错
        for bad in ("fast", "medium", "50k", "1000k", "", "unknown"):
            with self.assertRaises(ValueError):
                normalize_speed(bad)

    def test_parse_toml_profile_valid(self) -> None:
        """测试解析标准完整 12 通道 TOML 配置文件。"""
        toml_content = """
        [profile]
        name = "Avionics_6TX_6RX"
        version = "1.2"
        boot_slot = 1

        [global]
        mode = "ASCII"

        [channel.0]
        direction = "TX"
        speed = "high"

        [channel.1]
        direction = "RX"
        speed = "high"
        filter = ["ALLOW 310", "ALLOW 203"]

        [channel.2]
        direction = "TX"
        speed = "low"

        [channel.3]
        direction = "RX"
        speed = "low"
        filter = ["BLOCK 377"]

        [channel.4]
        direction = "DISABLED"
        speed = "high"
        """
        profile = parse_toml_profile(toml_content)
        self.assertEqual(profile["name"], "Avionics_6TX_6RX")
        self.assertEqual(profile["version"], "1.2")
        self.assertEqual(profile["boot_slot"], 1)
        self.assertEqual(profile["mode"], "ASCII")

        ch0 = profile["channels"][0]
        self.assertEqual(ch0["direction"], "TX")
        self.assertEqual(ch0["speed"], "HIGH")

        ch1 = profile["channels"][1]
        self.assertEqual(ch1["direction"], "RX")
        self.assertEqual(ch1["speed"], "HIGH")
        self.assertEqual(ch1["filter"], ["ALLOW 310", "ALLOW 203"])

        ch2 = profile["channels"][2]
        self.assertEqual(ch2["direction"], "TX")
        self.assertEqual(ch2["speed"], "LOW")

        ch3 = profile["channels"][3]
        self.assertEqual(ch3["direction"], "RX")
        self.assertEqual(ch3["speed"], "LOW")
        self.assertEqual(ch3["filter"], ["BLOCK 377"])

        ch4 = profile["channels"][4]
        self.assertEqual(ch4["direction"], "DISABLED")

    def test_parse_toml_profile_validation_errors(self) -> None:
        """测试 TOML 配置校验失败时的 Fail-Fast 机制。"""
        # 1. 语法错误
        with self.assertRaises(ValueError):
            parse_toml_profile("invalid toml content = [unclosed")

        # 2. 通道号超限 (12 > 11)
        with self.assertRaises(ValueError):
            parse_toml_profile("""
            [channel.12]
            direction = "TX"
            speed = "high"
            """)

        # 3. 通道标识非数字
        with self.assertRaises(ValueError):
            parse_toml_profile("""
            [channel.foo]
            direction = "TX"
            speed = "high"
            """)

        # 4. 无效通道工作方向
        with self.assertRaises(ValueError):
            parse_toml_profile("""
            [channel.0]
            direction = "DUPLEX"
            speed = "high"
            """)

        # 5. 无效 boot_slot 超限 (0..3)
        with self.assertRaises(ValueError):
            parse_toml_profile("""
            [profile]
            boot_slot = 5
            """)

    def test_build_batch_spec(self) -> None:
        """测试紧凑批处理配置指令生成。"""
        channels_cfg = {
            0: {"direction": "TX", "speed": "HIGH"},
            1: {"direction": "RX", "speed": "HIGH"},
            2: {"direction": "TX", "speed": "LOW"},
            3: {"direction": "RX", "speed": "LOW"},
        }
        spec = build_batch_spec(channels_cfg)
        parts = spec.split(",")
        self.assertEqual(len(parts), 12)
        self.assertEqual(parts[0], "0=TX:HIGH")
        self.assertEqual(parts[1], "1=RX:HIGH")
        self.assertEqual(parts[2], "2=TX:LOW")
        self.assertEqual(parts[3], "3=RX:LOW")
        # 4..11 未配置的应安全默认 DISABLED:HIGH
        for i in range(4, 12):
            self.assertEqual(parts[i], f"{i}=DISABLED:HIGH")

    def test_generate_toml_from_cfgdump_and_roundtrip(self) -> None:
        """测试从固件 CFGDUMP 响应生成 TOML 并完整无损回环反序列化。"""
        cfgdump_str = (
            "CFGDUMP: 0:TX:HIGH 1:RX:HIGH 2:RX:LOW 3:DISABLED:HIGH 4:TX:LOW "
            "5:RX:HIGH 6:DISABLED:LOW 7:TX:HIGH 8:RX:LOW 9:DISABLED:HIGH "
            "10:TX:HIGH 11:RX:HIGH"
        )
        toml_text = generate_toml_from_cfgdump(cfgdump_str, profile_name="Bench_Profile")
        self.assertIn('name = "Bench_Profile"', toml_text)
        self.assertIn("[channel.0]", toml_text)
        self.assertIn("[channel.11]", toml_text)

        # 回环解析
        parsed = parse_toml_profile(toml_text)
        self.assertEqual(parsed["name"], "Bench_Profile")
        self.assertEqual(len(parsed["channels"]), 12)

        self.assertEqual(parsed["channels"][0]["direction"], "TX")
        self.assertEqual(parsed["channels"][0]["speed"], "HIGH")

        self.assertEqual(parsed["channels"][1]["direction"], "RX")
        self.assertEqual(parsed["channels"][1]["speed"], "HIGH")

        self.assertEqual(parsed["channels"][2]["direction"], "RX")
        self.assertEqual(parsed["channels"][2]["speed"], "LOW")

        self.assertEqual(parsed["channels"][4]["direction"], "TX")
        self.assertEqual(parsed["channels"][4]["speed"], "LOW")

        self.assertEqual(parsed["channels"][6]["direction"], "DISABLED")
        self.assertEqual(parsed["channels"][6]["speed"], "LOW")

        self.assertEqual(parsed["channels"][11]["direction"], "RX")
        self.assertEqual(parsed["channels"][11]["speed"], "HIGH")

    def test_parse_profile_list_output(self) -> None:
        """测试解析固件片上 Flash PROFILE LIST 响应。"""
        firmware_lines = [
            "PROFILES: Boot Slot: Some(2)",
            '[Slot 0] ACTIVE "Bench_Default"',
            "[Slot 1] EMPTY",
            '[Slot 2] ACTIVE "Flight_Deck_12CH"',
            "[Slot 3] EMPTY",
        ]
        boot_slot, slots = parse_profile_list_output(firmware_lines)
        self.assertEqual(boot_slot, 2)
        self.assertEqual(len(slots), 4)

        self.assertEqual(slots[0]["slot"], 0)
        self.assertEqual(slots[0]["status"], "ACTIVE")
        self.assertEqual(slots[0]["name"], "Bench_Default")

        self.assertEqual(slots[1]["slot"], 1)
        self.assertEqual(slots[1]["status"], "EMPTY")

        self.assertEqual(slots[2]["slot"], 2)
        self.assertEqual(slots[2]["status"], "ACTIVE")
        self.assertEqual(slots[2]["name"], "Flight_Deck_12CH")

        self.assertEqual(slots[3]["slot"], 3)
        self.assertEqual(slots[3]["status"], "EMPTY")


if __name__ == "__main__":
    unittest.main()
