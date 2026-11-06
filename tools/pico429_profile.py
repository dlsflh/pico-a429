"""ARINC 429 12 通道配置方案与 TOML 编解码管理模块 (Configuration & Profile Management)"""

from __future__ import annotations

import tomllib
from typing import Any


def normalize_speed(speed_str: str) -> str:
    """标准化波特率名称为 HIGH 或 LOW (兼顾 ARINC 429 规范定义与历史别名)。"""
    s = speed_str.strip().lower()
    if s in ("high", "100k", "100", "100kbps"):
        return "HIGH"
    elif s in ("low", "12.5k", "12k5", "12.5", "12.5kbps"):
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

    global_info = data.get("global", {})
    mode = str(global_info.get("mode", "ASCII")).upper()
    if mode not in ("ASCII", "BIN"):
        mode = "ASCII"

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
        "mode": mode,
        "channels": channels,
    }


def build_batch_spec(channels: dict[int, dict[str, Any]]) -> str:
    """生成固件 CFGBATCH 紧凑配置字符串 (0=TX:HIGH,1=RX:HIGH,...)"""
    parts: list[str] = []
    for ch_idx in range(12):
        cfg = channels.get(ch_idx, {"direction": "DISABLED", "speed": "HIGH"})
        dir_str = cfg.get("direction", "DISABLED").upper()
        speed_str = cfg.get("speed", "HIGH").upper()
        parts.append(f"{ch_idx}={dir_str}:{speed_str}")
    return ",".join(parts)


def generate_toml_from_cfgdump(
    dump_line: str, profile_name: str = "Pico429_Profile"
) -> str:
    """从固件 CFGDUMP 响应生成标准的 TOML 配置文本。"""
    body = dump_line
    if "CFGDUMP:" in body:
        body = body.split("CFGDUMP:", 1)[1].strip()

    tokens = body.split()
    channels_map: dict[int, tuple[str, str]] = {}
    for tok in tokens:
        parts = tok.split(":")
        if len(parts) >= 2:
            try:
                ch = int(parts[0])
                direction = parts[1].upper()
                speed = parts[2].lower() if len(parts) >= 3 else "high"
                channels_map[ch] = (direction, speed)
            except ValueError:
                pass

    lines = [
        "# Pico-429 12-Channel ARINC 429 Configuration Profile",
        "# Generated automatically from hardware state",
        "[profile]",
        f'name = "{profile_name}"',
        'version = "1.0"',
        "",
        "[global]",
        'mode = "ASCII"',
        "",
    ]

    for ch in range(12):
        direction, speed = channels_map.get(ch, ("DISABLED", "high"))
        speed_norm = "high" if "high" in speed or "100" in speed else "low"
        lines.append(f"[channel.{ch}]")
        lines.append(f'direction = "{direction}"')
        lines.append(f'speed = "{speed_norm}"')
        if direction == "RX":
            lines.append('filter_mode = "none"')
            lines.append("labels = []")
        lines.append("")

    return "\n".join(lines)


def parse_profile_list_output(
    lines: list[str],
) -> tuple[int | None, list[dict[str, Any]]]:
    """解析固件 PROFILE LIST 响应。"""
    boot_slot: int | None = None
    slots: list[dict[str, Any]] = []

    for line in lines:
        if "Boot Slot:" in line:
            if "Some(" in line:
                try:
                    slot_str = line.split("Some(", 1)[1].split(")", 1)[0]
                    boot_slot = int(slot_str)
                except ValueError:
                    pass
        elif "[Slot" in line:
            try:
                parts = line.split("[Slot ", 1)[1]
                slot_id_str, rest = parts.split("]", 1)
                slot_id = int(slot_id_str)
                rest = rest.strip()
                if "ACTIVE" in rest:
                    name = ""
                    if '"' in rest:
                        name = rest.split('"', 1)[1].rsplit('"', 1)[0]
                    slots.append({"slot": slot_id, "status": "ACTIVE", "name": name})
                else:
                    slots.append({"slot": slot_id, "status": "EMPTY", "name": ""})
            except (ValueError, IndexError):
                pass

    return boot_slot, slots
