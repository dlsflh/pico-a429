"""ARINC 429 12 通道配置方案与 TOML 编解码管理模块 (Configuration & Profile Management)"""

from __future__ import annotations

import tomllib
from typing import Any


def normalize_speed(speed_str: str) -> str:
    """标准化波特率名称为 HIGH 或 LOW (支持 'high', '100k', 'low', '12.5k')。"""
    s = speed_str.strip().lower()
    if s in ("high", "100k", "100kbps"):
        return "HIGH"
    elif s in ("low", "12.5k", "12.5kbps"):
        return "LOW"
    raise ValueError(
        f"未知速率 '{speed_str}'。ARINC 429 仅支持 'high' (100 kbps) 或 'low' (12.5 kbps)。"
    )


def parse_toml_profile(toml_text: str) -> dict[str, Any]:
    """解析并校验 TOML 格式的 12 通道配置。"""
    try:
        data = tomllib.loads(toml_text)
    except (tomllib.TOMLDecodeError, ValueError) as e:
        raise ValueError(f"TOML 格式语法错误: {e}") from e

    profile_info = data.get("profile", {})
    name = str(profile_info.get("name", "Unnamed_Profile"))
    version = str(profile_info.get("version", "1.0"))
    boot_slot = profile_info.get("boot_slot")
    if boot_slot is not None:
        try:
            boot_slot = int(boot_slot)
            if not (0 <= boot_slot <= 3):
                raise ValueError(f"boot_slot 必须在 0..3 范围内，当前为: {boot_slot}")
        except ValueError as err:
            raise ValueError(f"无效的 boot_slot: {boot_slot}") from err

    raw_channels = data.get("channel", data.get("channels", {}))
    channels: dict[int, dict[str, Any]] = {}

    for ch_key, ch_data in raw_channels.items():
        try:
            ch_idx = int(ch_key)
        except ValueError:
            raise ValueError(f"通道标识必须为数字 (0..11)，当前为: '{ch_key}'")

        if not (0 <= ch_idx <= 11):
            raise ValueError(f"通道编号超限 (0..11): {ch_idx}")

        raw_dir = str(ch_data.get("direction", "DISABLED")).upper()
        if raw_dir not in ("TX", "RX", "DISABLED"):
            raise ValueError(
                f"CH{ch_idx} 无效工作方向 '{raw_dir}' (有效值: TX, RX, DISABLED)"
            )

        raw_speed = str(ch_data.get("speed", "high"))
        norm_speed = normalize_speed(raw_speed)

        if "filter" in ch_data:
            raise ValueError(
                f"CH{ch_idx} 包含已废弃的 'filter' 字段。请改用 'filter_mode' ('none' | 'whitelist' | 'blacklist') 与 'labels'。"
            )

        # 解析过滤模式与 Label 列表
        raw_filter_mode = str(ch_data.get("filter_mode", "none")).strip().lower()
        if not raw_filter_mode:
            raw_filter_mode = "none"

        if raw_filter_mode not in ("none", "whitelist", "blacklist"):
            raise ValueError(
                f"CH{ch_idx} 无效过滤模式 '{raw_filter_mode}' (有效值: 'none', 'whitelist', 'blacklist')"
            )

        raw_labels = ch_data.get("labels", [])
        if isinstance(raw_labels, (str, int)):
            raw_labels = [raw_labels]
        elif not isinstance(raw_labels, list):
            raw_labels = []

        # 校验 Label 合法性 (八进制 0..0o377)
        clean_labels: list[str] = []
        for lbl in raw_labels:
            lbl_str = str(lbl).strip()
            try:
                lbl_val = int(lbl_str, 8)
                if not (0 <= lbl_val <= 0o377):
                    raise ValueError
                clean_labels.append(f"{lbl_val:03o}")
            except ValueError:
                raise ValueError(
                    f"CH{ch_idx} 无效八进制 Label 数值 '{lbl_str}' (有效范围: 000~377)"
                )

        channels[ch_idx] = {
            "direction": raw_dir,
            "speed": norm_speed,
            "filter_mode": raw_filter_mode,
            "labels": clean_labels,
        }

    return {
        "name": name,
        "version": version,
        "boot_slot": boot_slot,
        "channels": channels,
    }
