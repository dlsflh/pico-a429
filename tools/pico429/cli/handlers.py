"""Pico-429 命令行交互与终端呈现处理器 (CLI Handlers Layer)

统一处理顶层指令 (ping, status, send, listen, bit, flash) 与 config 引擎。
"""

from __future__ import annotations

import argparse
import contextlib
import pathlib
import sys
import time
from collections.abc import Generator

from tools.arinc429_codec import (
    AckStatus,
    ArincWord,
    ParityMode,
    format_ack_status,
)
from tools.pico429.bit_engine import BitEngine
from tools.pico429.client import Pico429Client
from tools.pico429.profile import normalize_speed, parse_toml_profile
from tools.pico429.transport import SerialTransport, find_pico_port


@contextlib.contextmanager
def pico_client(
    port_arg: str | None, timeout: float = 0.5, op: str = "操作"
) -> Generator[Pico429Client]:
    """统一管理 Pico429 串口客户端连接生命周期与异常捕获。"""
    port = find_pico_port(port_arg)
    with Pico429Client(SerialTransport.open(port, timeout=timeout)) as client:
        try:
            yield client
        except (OSError, RuntimeError, TimeoutError, ValueError) as e:
            print(f"[\033[31mFAIL\033[0m] {op}失败: {e}", file=sys.stderr)
            sys.exit(1)


def cmd_ping(args: argparse.Namespace) -> None:
    port = find_pico_port(args.port)
    print(f"[*] 正在连接 {port} 并发送二进制 PING...")
    with pico_client(args.port, op="PING") as client:
        t0 = time.perf_counter()
        ack = client.send_cmd(0x01, timeout=1.0)
        rtt = (time.perf_counter() - t0) * 1000.0
        print(
            f"[\033[32mOK\033[0m] 收到下位机 PONG ACK (Seq {ack.seq_num}, RTT: {rtt:.2f} ms)"
        )


def cmd_status(args: argparse.Namespace) -> None:
    """展示 12 通道配置矩阵与当前运行模式。"""
    with pico_client(args.port, op="查询状态") as client:
        mode, channels = client.query_status(timeout=1.0)
        mode_str = (
            "\033[33mCONFIG\033[0m (维护模式: 物理总线静默，引脚高阻)"
            if mode == 0
            else "\033[32mOPERATIONAL\033[0m (运行模式: 物理总线实时收发)"
        )
        print(f"[\033[32mSTATUS\033[0m] 当前固件模式: {mode_str}")
        print("-" * 50)
        print(f"{'通道':<6} {'使能':<8} {'方向':<8} {'速率'}")
        print("-" * 50)
        for ch_idx, (en, is_rx, hs) in enumerate(channels):
            if getattr(args, "channel", None) is not None and ch_idx != args.channel:
                continue
            en_str = "\033[32mENABLE\033[0m" if en else "DISABLE"
            dir_str = ("RX" if is_rx else "TX") if en else "-"
            spd_str = ("100K" if hs else "12.5K") if en else "-"
            print(f"CH{ch_idx:<4} {en_str:<17} {dir_str:<8} {spd_str}")


def cmd_config_channel(args: argparse.Namespace) -> None:
    """配置单通道参数 (方向、速率、过滤规则)。"""
    ch = args.channel
    with pico_client(args.port, op="配置通道") as client:
        _, channels = client.query_status(timeout=1.0)
        _cur_en, cur_is_rx, cur_hs = channels[ch]

        if getattr(args, "disable", False):
            new_en = False
            new_is_rx = cur_is_rx
            new_hs = cur_hs
        else:
            new_en = True
            new_is_rx = args.dir.strip().upper() == "RX" if args.dir else cur_is_rx
            new_hs = normalize_speed(args.speed) == "HIGH" if args.speed else True

        ack = client.set_channel_cfg(ch, new_en, new_is_rx, new_hs)
        if ack.ack_status != AckStatus.OK:
            print(
                f"[\033[31mFAIL\033[0m] 配置 CH{ch} 失败: {format_ack_status(ack.ack_status)}",
                file=sys.stderr,
            )
            sys.exit(1)

        if new_en and new_is_rx:
            f_mode = args.filter.lower() if args.filter else "none"
            client.set_filter(ch, action_code=0)

            raw_labels: list[int] = []
            if args.labels:
                for token in args.labels.split(","):
                    t = token.strip()
                    if t:
                        try:
                            val = int(t, 8)
                            if not (0 <= val <= 0o377):
                                raise ValueError
                            raw_labels.append(val)
                        except ValueError:
                            print(
                                f"[\033[31mFAIL\033[0m] 无效八进制 Label '{t}' (有效范围 000~377)",
                                file=sys.stderr,
                            )
                            sys.exit(1)

            if f_mode == "whitelist":
                for lbl in raw_labels:
                    client.set_filter(ch, action_code=1, octal_label=lbl)
            elif f_mode == "blacklist":
                for lbl in raw_labels:
                    client.set_filter(ch, action_code=2, octal_label=lbl)

        if getattr(args, "run", False):
            client.set_operating_mode(1)
            print(
                f"[\033[32mOK\033[0m] 通道 CH{ch} 配置成功，固件已切换至 OPERATIONAL 运行模式"
            )
        else:
            dir_desc = ("RX" if new_is_rx else "TX") if new_en else "DISABLED"
            spd_desc = ("100K" if new_hs else "12.5K") if new_en else "-"
            print(
                f"[\033[32mOK\033[0m] 通道 CH{ch} 配置成功: 使能={new_en}, 方向={dir_desc}, 速率={spd_desc}"
            )


def cmd_config_mode(args: argparse.Namespace) -> None:
    """查询或切换固件工作模式 (CONFIG / RUN)。"""
    with pico_client(args.port, op="模式切换") as client:
        target = args.target_mode
        if target is None or target.lower() == "status":
            mode, _ = client.query_status()
            desc = (
                "CONFIG (维护静默模式)" if mode == 0 else "OPERATIONAL (实时运行模式)"
            )
            print(f"[*] 当前固件运行模式: {desc}")
            return

        to_operational = target.lower() in ("run", "operational")
        ack = client.set_operating_mode(1 if to_operational else 0)
        if ack.ack_status == AckStatus.OK:
            desc = (
                "\033[32mOPERATIONAL (实时运行模式)\033[0m"
                if to_operational
                else "\033[33mCONFIG (维护静默模式)\033[0m"
            )
            print(f"[\033[32mOK\033[0m] 固件运行模式已成功切换为: {desc}")
        else:
            print(
                f"[\033[31mFAIL\033[0m] 模式切换失败: {format_ack_status(ack.ack_status)}",
                file=sys.stderr,
            )
            sys.exit(1)


def cmd_flash_apply(args: argparse.Namespace) -> None:
    """从本地 TOML 文件批量应用配置到硬件，支持可选固化至 Flash 槽位。"""
    path = pathlib.Path(args.file)
    if not path.exists():
        print(f"错误: 配置文件不存在: {path}", file=sys.stderr)
        sys.exit(1)

    parsed = parse_toml_profile(path.read_text(encoding="utf-8"))
    prof_name = parsed.get("name", "Unnamed_Profile")
    port = find_pico_port(args.port)
    print(f"[*] 正在向 {port} 应用配置方案 '{prof_name}'...")

    with pico_client(args.port, op="应用配置") as client:
        client.set_operating_mode(0)

        channels_list: list[tuple[bool, bool, bool]] = []
        for ch in range(12):
            cfg = parsed["channels"].get(ch, {})
            d = cfg.get("direction", "DISABLED").upper()
            en = d in ("TX", "RX")
            is_rx = d == "RX"
            s = cfg.get("speed", "HIGH").upper()
            hs = "HIGH" in s or "100" in s
            channels_list.append((en, is_rx, hs))

        print("[*] 批量安全下发 12 通道配置 (CFGBATCH)...")
        ack = client.cfg_batch(channels_list)
        if ack.ack_status != AckStatus.OK:
            print(
                f"[\033[31mFAIL\033[0m] CFGBATCH 下发失败: {format_ack_status(ack.ack_status)}",
                file=sys.stderr,
            )
            sys.exit(1)

        for ch, ch_cfg in parsed["channels"].items():
            if ch_cfg["direction"] != "RX":
                continue
            f_mode = ch_cfg.get("filter_mode", "none").lower()
            labels = ch_cfg.get("labels", [])
            client.set_filter(ch, action_code=0)

            if f_mode == "whitelist":
                for lbl_str in labels:
                    client.set_filter(ch, action_code=1, octal_label=int(lbl_str, 8))
            elif f_mode == "blacklist":
                for lbl_str in labels:
                    client.set_filter(ch, action_code=2, octal_label=int(lbl_str, 8))

        slot_id = getattr(args, "slot", None)
        if slot_id is not None:
            print(f"[*] 同步固化配置方案 '{prof_name}' 至 Flash Slot {slot_id}...")
            save_ack = client.profile_save_with_name(slot_id, prof_name)
            if save_ack.ack_status == AckStatus.OK:
                print(
                    f"[\033[32mOK\033[0m] 方案 '{prof_name}' 已成功持久化写入 Flash Slot {slot_id}"
                )
            else:
                print(
                    f"[\033[31mWARNING\033[0m] 固化写入 Flash Slot {slot_id} 失败: {format_ack_status(save_ack.ack_status)}"
                )

        if not getattr(args, "no_run", False):
            client.set_operating_mode(1)
            print(
                "[\033[32mOK\033[0m] 12 通道配置应用成功，固件已切换至 OPERATIONAL 运行模式"
            )
        else:
            print("[\033[32mOK\033[0m] 12 通道配置应用成功，固件保持在 CONFIG 维护模式")


def cmd_flash_list(args: argparse.Namespace) -> None:
    """列出片上 Flash 中的非易失配置方案列表 (包含方案名称与自启动状态)。"""
    with pico_client(args.port, op="查询 Flash 配置列表") as client:
        acks = client.profile_list()
        print("\n" + "=" * 65)
        print("片上 Flash 非易失存储配置方案列表 (Slot 0..3):")
        print("-" * 65)
        print(f"{'槽位':<6} {'自启动':<8} {'状态':<10} {'校验':<10} {'方案名称'}")
        print("-" * 65)

        boot_slot = None
        for ack in acks:
            slot_id = ack.channel
            is_valid = (ack.payload_0 & 0xFF) != 0
            is_boot = ((ack.payload_0 >> 8) & 0xFF) != 0
            crc16_val = (ack.payload_0 >> 16) & 0xFFFF
            if is_boot:
                boot_slot = slot_id
            boot_tag = "  [*]" if is_boot else "   -"
            status_tag = "ACTIVE" if is_valid else "EMPTY"
            crc_tag = f"0x{crc16_val:04X}" if is_valid else "-"
            name_tag = f'"{client.get_profile_name(slot_id)}"' if is_valid else "-"
            print(
                f"Slot {slot_id:<3} {boot_tag:<8} {status_tag:<10} {crc_tag:<10} {name_tag}"
            )

        print("=" * 65)
        if boot_slot is not None:
            print(f"[*] 当前上电自启动槽位: Slot {boot_slot}")
        else:
            print(
                "[*] 当前无自启动槽位 (默认静默基态: 12 通道全部 Disabled，模式 CONFIG)"
            )


def cmd_flash_load(args: argparse.Namespace) -> None:
    slot = args.slot
    with pico_client(args.port, op="装载配置") as client:
        ack = client.profile_load(slot)
        if ack.ack_status == AckStatus.OK:
            name = client.get_profile_name(slot)
            print(
                f"[\033[32mOK\033[0m] Flash Slot {slot} ('{name}') 配置已装载，固件已切换至 OPERATIONAL 运行模式"
            )
        else:
            print(
                f"[\033[31mFAIL\033[0m] 装载 Slot {slot} 失败: {format_ack_status(ack.ack_status)}",
                file=sys.stderr,
            )
            sys.exit(1)


def cmd_flash_boot(args: argparse.Namespace) -> None:
    with pico_client(args.port, op="自启动配置") as client:
        mode, _ = client.query_status()
        if mode != 0:
            client.set_operating_mode(0)

        if getattr(args, "clear", False):
            ack = client.profile_clear_boot()
            if ack.ack_status == AckStatus.OK:
                print(
                    "[\033[32mOK\033[0m] 已清除开机自启动设置，上电将默认处于 CONFIG 静默基态"
                )
            else:
                print(
                    f"[\033[31mFAIL\033[0m] 清除自启动失败: {format_ack_status(ack.ack_status)}",
                    file=sys.stderr,
                )
                sys.exit(1)
        else:
            slot = args.slot
            if slot is None:
                print(
                    "错误: 必须指定 slot 槽位号 (0..3) 或使用 --clear", file=sys.stderr
                )
                sys.exit(1)
            ack = client.profile_boot(slot)
            if ack.ack_status == AckStatus.OK:
                print(f"[\033[32mOK\033[0m] 已将 Flash Slot {slot} 设为开机自启动方案")
            else:
                print(
                    f"[\033[31mFAIL\033[0m] 设置自启动失败: {format_ack_status(ack.ack_status)}",
                    file=sys.stderr,
                )
                sys.exit(1)


def cmd_flash_clear(args: argparse.Namespace) -> None:
    """清空 Flash 槽位配置 (单个槽位或 --all 全部清空)。"""
    with pico_client(args.port, op="清空 Flash 配置") as client:
        mode, _ = client.query_status()
        if mode != 0:
            client.set_operating_mode(0)

        if getattr(args, "all", False):
            ack = client.profile_clear_all()
            if ack.ack_status == AckStatus.OK:
                print(
                    "[\033[32mOK\033[0m] 已清空 Flash 全部 4 个槽位及开机自启动配置！"
                )
            else:
                print(
                    f"[\033[31mFAIL\033[0m] 全清 Flash 失败: {format_ack_status(ack.ack_status)}",
                    file=sys.stderr,
                )
                sys.exit(1)
        else:
            slot = args.slot
            if slot is None:
                print(
                    "错误: 必须指定 slot 槽位号 (0..3) 或使用 --all 清空全部",
                    file=sys.stderr,
                )
                sys.exit(1)
            ack = client.profile_clear(slot)
            if ack.ack_status == AckStatus.OK:
                print(f"[\033[32mOK\033[0m] 已成功清空 Flash Slot {slot} 配置！")
            else:
                print(
                    f"[\033[31mFAIL\033[0m] 清空 Slot {slot} 失败: {format_ack_status(ack.ack_status)}",
                    file=sys.stderr,
                )
                sys.exit(1)


def _format_slot_toml(name: str, channels: list[tuple[bool, bool, bool]]) -> str:
    lines = [
        "# Pico-429 12-Channel ARINC 429 Configuration Profile (Dump from Flash)",
        "[profile]",
        f'name = "{name}"',
        'version = "1.0"',
        "",
    ]
    for ch, (en, is_rx, hs) in enumerate(channels):
        d = ("RX" if is_rx else "TX") if en else "DISABLED"
        s = "high" if hs else "low"
        lines.append(f"[channel.{ch}]")
        lines.append(f'direction = "{d}"')
        lines.append(f'speed = "{s}"')
        if is_rx and en:
            lines.append('filter_mode = "none"')
            lines.append("labels = []")
        lines.append("")
    return "\n".join(lines)


def cmd_flash_dump(args: argparse.Namespace) -> None:
    """读取片上 Flash 槽位配置并导出为 TOML 文件。"""
    with pico_client(args.port, op="导出 Flash 配置") as client:
        if getattr(args, "all", False):
            acks = client.profile_list()
            exported_count = 0
            out_dir = pathlib.Path(args.out) if args.out else pathlib.Path("profiles")
            out_dir.mkdir(parents=True, exist_ok=True)

            for ack in acks:
                s_id = ack.channel
                if (ack.payload_0 & 0xFF) == 0:
                    continue
                ok, channels = client.profile_get_channels(s_id)
                if not ok:
                    try:
                        client.profile_load(s_id)
                        _, channels = client.query_status()
                        ok = True
                    except (OSError, RuntimeError, TimeoutError, ValueError):
                        ok = False
                if not ok:
                    continue

                name_str = client.get_profile_name(s_id)
                toml_str = _format_slot_toml(name_str, channels)
                target_file = out_dir / f"slot{s_id}.toml"
                target_file.write_text(toml_str, encoding="utf-8")
                print(
                    f"[\033[32mOK\033[0m] Slot {s_id} ('{name_str}') 已成功导出至: {target_file}"
                )
                exported_count += 1

            if exported_count == 0:
                print("提示: 片上 Flash 中暂无有效配置方案。")
        else:
            slot = args.slot
            if slot is None:
                print(
                    "错误: 必须指定 slot 槽位号 (0..3) 或使用 --all 导出全部",
                    file=sys.stderr,
                )
                sys.exit(1)
            ok, channels = client.profile_get_channels(slot)
            if not ok:
                try:
                    client.profile_load(slot)
                    _, channels = client.query_status()
                    ok = True
                except (OSError, RuntimeError, TimeoutError, ValueError):
                    ok = False
            if not ok:
                print(
                    f"[\033[31mFAIL\033[0m] Flash Slot {slot} 为空或损坏，无法导出",
                    file=sys.stderr,
                )
                sys.exit(1)
            name_str = client.get_profile_name(slot)
            toml_str = _format_slot_toml(name_str, channels)
            if args.out:
                out_path = pathlib.Path(args.out)
                out_path.write_text(toml_str, encoding="utf-8")
                print(
                    f"[\033[32mOK\033[0m] Slot {slot} ('{name_str}') 配置已导出至: {out_path}"
                )
            else:
                print(toml_str)


def cmd_send(args: argparse.Namespace) -> None:
    with pico_client(args.port, op="发送") as client:
        mode, channels = client.query_status()
        if mode == 0:
            print(
                "[\033[31mFAIL\033[0m] 设备当前处于 CONFIG 维护模式，拒绝发送数据字！\n"
                "请先通过 'pico429 config mode run' 切换至运行模式。",
                file=sys.stderr,
            )
            sys.exit(1)

        ch = args.channel
        en, is_rx, _ = channels[ch]
        if not en or is_rx:
            print(
                f"[\033[31mFAIL\033[0m] CH{ch} 未使能为 TX 发送通道 (状态: en={en}, dir={'RX' if is_rx else 'TX'})！",
                file=sys.stderr,
            )
            sys.exit(1)

        if args.raw:
            raw_val = int(args.raw, 0)
            word = ArincWord(raw_val)
        else:
            if args.label is None or args.data is None:
                print(
                    "错误: 必须指定 --raw 原始十六进制字，或者同时提供 --label 和 --data",
                    file=sys.stderr,
                )
                sys.exit(1)
            lbl = int(args.label, 8)
            data = int(args.data, 0)
            parity = ParityMode(args.parity.lower())
            word = ArincWord.from_fields(lbl, args.sdi, data, args.ssm, parity)

        ack = client.send_word(ch, word.raw)
        if ack.ack_status == AckStatus.OK:
            print(
                f"[\033[32mOK\033[0m] CH{ch} 发送成功: L:{word.label_str} SDI:{word.sdi} "
                f"DATA:0x{word.data:05X} SSM:{word.ssm} (RAW: 0x{word.raw:08X})"
            )
        else:
            print(
                f"[\033[31mFAIL\033[0m] 发送被下位机拒绝: {format_ack_status(ack.ack_status)}",
                file=sys.stderr,
            )
            sys.exit(1)


def cmd_bit(args: argparse.Namespace) -> None:
    with pico_client(args.port, op="BIT自检") as client:
        if getattr(args, "profile", None):
            prof_path = pathlib.Path(args.profile)
            if not prof_path.exists():
                print(
                    f"[\033[31mFAIL\033[0m] 指定的配置文件不存在: {prof_path}",
                    file=sys.stderr,
                )
                sys.exit(1)
            print(f"[*] 正在装载 BIT 测试配置方案: {prof_path}...")
            parsed = parse_toml_profile(prof_path.read_text(encoding="utf-8"))
            channels_list: list[tuple[bool, bool, bool]] = []
            for ch in range(12):
                cfg = parsed["channels"].get(ch, {})
                d = cfg.get("direction", "DISABLED").upper()
                en = d in ("TX", "RX")
                is_rx = d == "RX"
                s = cfg.get("speed", "HIGH").upper()
                hs = "HIGH" in s or "100" in s
                channels_list.append((en, is_rx, hs))
            client.cfg_batch(channels_list)
            client.set_operating_mode(1)

        tx_ch = getattr(args, "tx", 0)
        rx_ch = getattr(args, "rx", 1)

        print(f"[*] 正在启动内置自测试 (BIT: CH{tx_ch} TX -> CH{rx_ch} RX)...")
        report = BitEngine.run_bit(client, tx_ch=tx_ch, rx_ch=rx_ch)

        if report.error_message:
            print(
                f"\n[\033[31mBIT REJECTED\033[0m] {report.error_message}",
                file=sys.stderr,
            )
            sys.exit(1)

        print("\n" + "=" * 70)
        print("  Pico-429 8 组基准向量闭环自检结果 (Built-In-Test Report)")
        print("=" * 70)
        print(f"发送通道: CH{report.tx_ch} | 接收通道: CH{report.rx_ch}")
        print("-" * 70)
        print(
            f"{'序号':<4} {'Label':<6} {'期望 RAW':<12} {'捕获 RAW':<12} {'耗时(ms)':<10} {'状态':<6} {'测试用例'}"
        )
        print("-" * 70)

        for vec in report.vectors:
            status_tag = "\033[32mPASS\033[0m" if vec.matched else "\033[31mFAIL\033[0m"
            rtt_str = f"{vec.rtt_ms:.2f}" if vec.rtt_ms > 0 else "-"
            rx_hex = f"0x{vec.raw_rx:08X}" if vec.raw_rx > 0 else "-"
            print(
                f"{vec.index:<4} {vec.label_octal:03o}    0x{vec.raw_tx:08X} {rx_hex:<12} {rtt_str:<10} {status_tag:<15} {vec.description}"
            )

        print("=" * 70)
        if report.passed:
            print(
                "[\033[32mOVERALL PASS\033[0m] 8 组基准向量全部通过硬件物理闭环校验！"
            )
        else:
            print("[\033[31mOVERALL FAIL\033[0m] 存在回环失败向量，请检查物理连线！")
            sys.exit(1)


def handle_cli(args: argparse.Namespace) -> None:
    flash_actions = {
        "apply": cmd_flash_apply,
        "load": cmd_flash_load,
        "dump": cmd_flash_dump,
        "list": cmd_flash_list,
        "boot": cmd_flash_boot,
        "clear": cmd_flash_clear,
    }

    if args.command == "ping":
        cmd_ping(args)
    elif args.command == "status":
        cmd_status(args)
    elif args.command == "send":
        cmd_send(args)
    elif args.command == "listen":
        from tools.pico429.cli.listen import cmd_listen

        cmd_listen(args)
    elif args.command == "bit":
        cmd_bit(args)
    elif args.command == "flash":
        act = args.flash_action
        if act in flash_actions:
            flash_actions[act](args)
    elif args.command == "config":
        sub = args.config_subcommand
        if sub == "channel":
            cmd_config_channel(args)
        elif sub == "mode":
            cmd_config_mode(args)
        elif sub == "flash":
            act = args.flash_action
            if act in flash_actions:
                flash_actions[act](args)
