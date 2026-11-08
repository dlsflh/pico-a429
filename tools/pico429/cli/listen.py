"""Pico-429 专职总线遥测监听与抓包器 (Dedicated Bus Sniffer & Listener)

专注于 ARINC 429 物理总线遥测流的高性能监听与彩色格式化呈现：
- 严格遵循 CONFIG 模式静默基态约束 (CONFIG 模式下拒绝进入)
- 零线程竞态、零终端 UI 抢占，微秒级相对耗时精准呈现
- 支持单通道过滤监听与全通道汇聚监听
"""

from __future__ import annotations

import argparse
import sys
import time

from tools.arinc429_codec import (
    ArincWord,
    FrameFlags,
    FrameType,
    find_frame_in_stream,
)
from tools.pico429.client import Pico429Client
from tools.pico429.transport import SerialTransport, find_pico_port


def cmd_listen(args: argparse.Namespace) -> None:
    port = find_pico_port(args.port)
    print(f"[*] 正在连接 {port} 并校验固件模式...")

    with Pico429Client(SerialTransport.open(port, timeout=0.5)) as client:
        try:
            mode, channels = client.query_status(timeout=1.0)
        except (OSError, RuntimeError, TimeoutError, ValueError) as e:
            print(f"[\033[31mFAIL\033[0m] 查询固件状态失败: {e}", file=sys.stderr)
            sys.exit(1)

        if mode == 0:
            print(
                "\n[\033[31mERROR\033[0m] 当前固件处于 CONFIG 静默维护模式，总线收发已锁定！\n"
                "固件在此模式下保持物理总线静默与引脚高阻。\n"
                "请先通过以下方式之一进入 OPERATIONAL 模式：\n"
                "  1. 激活运行模式: uv run python tools/pico429.py config mode run\n"
                "  2. 应用测试方案: uv run python tools/pico429.py flash apply profiles/loopback_ch0_ch1.toml\n"
                "  3. 加载 Flash 槽位: uv run python tools/pico429.py flash load 0\n",
                file=sys.stderr,
            )
            sys.exit(1)

        target_ch = getattr(args, "channel", None)
        active_rx = [ch for ch, (en, rx, _) in enumerate(channels) if en and rx]

        if target_ch is not None:
            if target_ch not in active_rx:
                print(
                    f"[\033[33mWARNING\033[0m] 通道 CH{target_ch} 当前未使能为 RX 接收通道！"
                )
            filter_desc = f"仅监听指定通道 CH{target_ch}"
        else:
            filter_desc = (
                f"监听全部活跃 RX 通道: {', '.join(f'CH{c}' for c in active_rx)}"
                if active_rx
                else "当前无使能的 RX 接收通道"
            )

        print("\n" + "=" * 70)
        print("  Pico-429 专用总线遥测流监听终端 (ARINC 429 Bus Monitor)")
        print("=" * 70)
        print(f"[*] 串口: {port} | 固件工作模式: OPERATIONAL (实时收发)")
        print(f"[*] 监听范围: {filter_desc}")
        print("[*] 正在等待总线到达数据帧 (按 Ctrl+C 退出监听)...")
        print("-" * 70)

        stream_buf = bytearray()
        t_start = time.perf_counter()
        count_limit = getattr(args, "count", None)
        timeout_limit = getattr(args, "timeout", None)

        received_count = 0
        first_fw_us: int | None = None
        last_raw_fw_us: int = 0
        fw_epoch_us: int = 0

        try:
            while True:
                if (
                    timeout_limit is not None
                    and (time.perf_counter() - t_start) >= timeout_limit
                ):
                    print(f"[*] 已达到最长监听时间限制 ({timeout_limit}s)，自动退出。")
                    break

                chunk = client.transport.recv(64)
                if chunk:
                    stream_buf.extend(chunk)
                    while True:
                        res = find_frame_in_stream(stream_buf)
                        if res is None:
                            break
                        frame, consumed = res
                        del stream_buf[:consumed]

                        if frame.frame_type == FrameType.ARINC_DATA:
                            if target_ch is not None and frame.channel != target_ch:
                                continue

                            # 固件硬件时间戳解析与 32 位溢出回绕展开
                            raw_fw_us = frame.payload_0
                            if first_fw_us is None:
                                first_fw_us = raw_fw_us
                                last_raw_fw_us = raw_fw_us
                                fw_epoch_us = 0
                            else:
                                if (
                                    raw_fw_us < last_raw_fw_us
                                    and (last_raw_fw_us - raw_fw_us) > 0x80000000
                                ):
                                    fw_epoch_us += 0x100000000
                                last_raw_fw_us = raw_fw_us

                            fw_total_us = fw_epoch_us + raw_fw_us
                            fw_rel_ms = (fw_total_us - first_fw_us) / 1000.0

                            word = ArincWord(frame.payload_1)
                            parity_ok = (frame.flags & FrameFlags.PARITY_ERROR) == 0
                            p_str = (
                                "\033[32mOK\033[0m"
                                if parity_ok
                                else "\033[31mFAIL(奇偶错)\033[0m"
                            )

                            print(
                                f"[+] [\033[32mRX CH{frame.channel}\033[0m] "
                                f"L:{word.label_str} SDI:{word.sdi} DATA:0x{word.data:05X} ({word.data}) "
                                f"SSM:{word.ssm} P:{p_str} (RAW: 0x{word.raw:08X}, FW: +{fw_rel_ms:.3f}ms)"
                            )
                            received_count += 1
                            if (
                                count_limit is not None
                                and received_count >= count_limit
                            ):
                                print(
                                    f"[*] 已成功捕获指定帧数 ({count_limit} 帧)，自动退出。"
                                )
                                return
                else:
                    time.sleep(0.002)

        except KeyboardInterrupt:
            print("\n[*] 正在退出总线监听...")
        finally:
            print("[*] 监听已安全退出。")
