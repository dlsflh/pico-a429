"""Pico-429 命令行参数解析器 (CLI Argument Parser)

精简后的命令体系：
- 顶层指令：ping, status, send, listen, bit, flash, config
- config 子命令：channel, mode, flash
- flash 子命令：apply, load, dump, list, boot, clear
"""

from __future__ import annotations

import argparse


def _setup_flash_subparsers(flash_sub: argparse._SubParsersAction) -> None:  # type: ignore[type-arg]
    """注册 flash 子命令 (共享给顶层 flash 与 config flash)。"""
    # flash apply <file> [--slot 0..3] [--no-run]
    p_fl_apply = flash_sub.add_parser(
        "apply",
        help="从本地 TOML 文件批量应用配置到硬件 (可选写入 Flash 槽位)",
    )
    p_fl_apply.add_argument("file", help="TOML 配置文件路径")
    p_fl_apply.add_argument("--port", "-p", help="串口名称")
    p_fl_apply.add_argument(
        "--slot",
        type=int,
        choices=range(4),
        help="可选同时固化写入片上 Flash 槽位 (0..3，包含配置名称)",
    )
    p_fl_apply.add_argument(
        "--no-run",
        action="store_true",
        help="配置完成后保持在 CONFIG 维护模式 (默认自动进入 OPERATIONAL)",
    )

    # flash load <slot>
    p_fl_load = flash_sub.add_parser(
        "load", help="从片上 Flash 槽位加载配置并激活为 OPERATIONAL 模式"
    )
    p_fl_load.add_argument("--port", "-p", help="串口名称")
    p_fl_load.add_argument("slot", type=int, choices=range(4), help="源槽位号 (0..3)")

    # flash dump [slot] [--all] [--out <file>]
    p_fl_dump = flash_sub.add_parser(
        "dump",
        help="读取片上 Flash 配置导出为 TOML 配置文件 (支持单个槽位或 --all)",
    )
    p_fl_dump.add_argument("--port", "-p", help="串口名称")
    p_fl_dump.add_argument(
        "slot",
        type=int,
        nargs="?",
        choices=range(4),
        help="源槽位号 (0..3，使用 --all 时省略)",
    )
    p_fl_dump.add_argument(
        "--all", action="store_true", help="导出 Flash 中所有已激活的槽位"
    )
    p_fl_dump.add_argument(
        "--out", "-o", help="输出文件路径或目标文件夹 (导出单文件或多文件)"
    )

    # flash list
    p_fl_list = flash_sub.add_parser(
        "list",
        help="列出片上 Flash 中的所有槽位方案名称、状态与自启动标记",
    )
    p_fl_list.add_argument("--port", "-p", help="串口名称")

    # flash boot <slot> [--clear]
    p_fl_boot = flash_sub.add_parser("boot", help="设置或清除开机上电自启动槽位")
    p_fl_boot.add_argument("--port", "-p", help="串口名称")
    p_fl_boot.add_argument(
        "slot",
        type=int,
        nargs="?",
        choices=range(4),
        help="自启动槽位号 (0..3，清除时省略)",
    )
    p_fl_boot.add_argument(
        "--clear",
        action="store_true",
        help="清空上电自启动 (恢复上电默认静默基态)",
    )

    # flash clear [slot] [--all]
    p_fl_clear = flash_sub.add_parser(
        "clear", help="清空 Flash 槽位配置 (支持单个槽位或 --all 全清)"
    )
    p_fl_clear.add_argument("--port", "-p", help="串口名称")
    p_fl_clear.add_argument(
        "slot",
        type=int,
        nargs="?",
        choices=range(4),
        help="目标槽位号 (0..3，使用 --all 时省略)",
    )
    p_fl_clear.add_argument(
        "--all", action="store_true", help="清空所有 4 个槽位及自启动设置"
    )


def create_parser() -> argparse.ArgumentParser:
    """构建精简顶层 CLI 参数解析树。"""
    parser = argparse.ArgumentParser(
        prog="pico429",
        description="ARINC 429 Pico 2 / Pico 2 W 极速免编译交互与诊断工具 (Zero-Compile Diagnostic Tool)",
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    parser.add_argument("--port", "-p", help="串口名称 (省略时自动检索 Pico 设备)")

    subparsers = parser.add_subparsers(dest="command", required=True)

    # -------------------------------------------------------------------------
    # 1. ping
    # -------------------------------------------------------------------------
    p_ping = subparsers.add_parser("ping", help="测试与 Pico 固件的物理/协议连通性")
    p_ping.add_argument("--port", "-p", help="串口名称")

    # -------------------------------------------------------------------------
    # 2. status (顶层独立命令：查看 12 通道配置矩阵与当前运行模式)
    # -------------------------------------------------------------------------
    p_status = subparsers.add_parser(
        "status", help="查询 12 通道物理使能、方向、波特率矩阵与工作模式"
    )
    p_status.add_argument("--port", "-p", help="串口名称")
    p_status.add_argument(
        "--channel",
        "-c",
        type=int,
        choices=range(12),
        help="仅展示指定单个通道",
    )

    # -------------------------------------------------------------------------
    # 3. send
    # -------------------------------------------------------------------------
    p_send = subparsers.add_parser("send", help="通过指定发送通道发送 ARINC 429 字")
    p_send.add_argument("--port", "-p", help="串口名称")
    p_send.add_argument(
        "--channel",
        "-c",
        type=int,
        default=0,
        choices=range(12),
        help="发送通道 (默认 0)",
    )
    p_send.add_argument("--label", help="八进制 Label (000~377，如 310)")
    p_send.add_argument(
        "--sdi",
        type=int,
        default=0,
        choices=range(4),
        help="源/目标标识符 SDI (0~3)",
    )
    p_send.add_argument("--data", help="19 位数据负载 (十六进制如 0x1234 或十进制)")
    p_send.add_argument(
        "--ssm",
        type=int,
        default=3,
        choices=range(4),
        help="状态符号位 SSM (0~3)",
    )
    p_send.add_argument(
        "--parity",
        choices=["odd", "even", "none"],
        default="odd",
        help="奇偶校验模式 (默认 odd)",
    )
    p_send.add_argument("--raw", help="32 位原始十六进制字 (如 0xE048D013)")

    # -------------------------------------------------------------------------
    # 4. listen (专职总线遥测流监听终端)
    # -------------------------------------------------------------------------
    p_listen = subparsers.add_parser(
        "listen", help="持续监听并彩色格式化输出 ARINC 429 总线遥测流"
    )
    p_listen.add_argument("--port", "-p", help="串口名称")
    p_listen.add_argument(
        "--channel",
        "-c",
        type=int,
        choices=range(12),
        help="限定仅监听指定接收通道 (省略时监听全部活跃 RX 通道)",
    )
    p_listen.add_argument(
        "--count",
        type=int,
        default=None,
        help="接收指定帧数后自动退出 (可选)",
    )
    p_listen.add_argument(
        "--timeout",
        type=float,
        default=None,
        help="最长监听等待时间 (秒，可选)",
    )

    # -------------------------------------------------------------------------
    # 5. bit (内置自测试，严格显式校验，支持 --profile 预装载)
    # -------------------------------------------------------------------------
    p_bit = subparsers.add_parser("bit", help="执行内置自测试 (Built-In-Test)")
    p_bit.add_argument("--port", "-p", help="串口名称")
    p_bit.add_argument("--tx", type=int, default=0, help="发送通道 (默认 0)")
    p_bit.add_argument("--rx", type=int, default=1, help="接收通道 (默认 1)")
    p_bit.add_argument(
        "--profile",
        help="预先装载的测试 TOML 配置文件路径 (例如 profiles/loopback_ch0_ch1.toml)",
    )

    # -------------------------------------------------------------------------
    # 6. flash (顶层便捷入口与 config flash 共享子命令体系)
    # -------------------------------------------------------------------------
    p_flash = subparsers.add_parser(
        "flash",
        help="片上 Flash 非易失存储管理 (方案应用、槽位固化、加载、转储、清空)",
    )
    p_flash.add_argument("--port", "-p", help="串口名称")
    flash_sub = p_flash.add_subparsers(dest="flash_action", required=True)
    _setup_flash_subparsers(flash_sub)

    # -------------------------------------------------------------------------
    # 7. config (硬件单通道与模式配置引擎)
    # -------------------------------------------------------------------------
    p_config = subparsers.add_parser(
        "config",
        help="硬件通道与工作模式统一配置引擎",
    )
    p_config.add_argument("--port", "-p", help="串口名称")
    config_sub = p_config.add_subparsers(dest="config_subcommand", required=True)

    # 7.1 config channel <ch> [options]
    p_cfg_ch = config_sub.add_parser(
        "channel",
        help="配置单通道参数 (方向、速率、过滤规则)",
    )
    p_cfg_ch.add_argument("--port", "-p", help="串口名称")
    p_cfg_ch.add_argument(
        "channel", type=int, choices=range(12), help="通道编号 (0..11)"
    )
    p_cfg_ch.add_argument(
        "--dir",
        choices=["TX", "RX", "tx", "rx"],
        help="工作方向 (TX / RX)",
    )
    p_cfg_ch.add_argument(
        "--disable",
        action="store_true",
        help="停用该通道 (切换为高阻弱下拉静默状态)",
    )
    p_cfg_ch.add_argument(
        "--speed",
        choices=["high", "low", "100k", "12.5k", "12k5"],
        help="波特率 (默认: high / 100kbps)",
    )
    p_cfg_ch.add_argument(
        "--filter",
        choices=["none", "whitelist", "blacklist"],
        help="过滤模式 (默认: none 全放行)",
    )
    p_cfg_ch.add_argument(
        "--labels",
        help="八进制 Label 列表 (逗号分隔，如 '310,203')；未指定时白名单全阻断，黑名单全放行",
    )
    p_cfg_ch.add_argument(
        "--run",
        action="store_true",
        help="配置完成后自动激活至 OPERATIONAL 运行模式",
    )

    # 7.2 config mode [config|run]
    p_cfg_mode = config_sub.add_parser(
        "mode",
        help="查询或切换固件运行模式 (CONFIG 维护静默模式 / RUN 运行模式)",
    )
    p_cfg_mode.add_argument("--port", "-p", help="串口名称")
    p_cfg_mode.add_argument(
        "target_mode",
        nargs="?",
        choices=[
            "config",
            "run",
            "operational",
            "CONFIG",
            "RUN",
            "OPERATIONAL",
            "status",
        ],
        help="目标模式 (省略或 status 时仅查询当前运行模式)",
    )

    # 7.3 config flash (同时挂载至 config 下)
    p_cfg_flash = config_sub.add_parser(
        "flash", help="片上 Flash 非易失存储管理 (flash 别名)"
    )
    p_cfg_flash.add_argument("--port", "-p", help="串口名称")
    cfg_flash_sub = p_cfg_flash.add_subparsers(dest="flash_action", required=True)
    _setup_flash_subparsers(cfg_flash_sub)

    return parser
