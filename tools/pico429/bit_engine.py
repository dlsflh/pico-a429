"""ARINC 429 内置自测试 (BIT) 核心算法与自动化检测引擎 (Diagnostic & BIT Engine)"""

from __future__ import annotations

import time
from dataclasses import dataclass
from typing import TYPE_CHECKING

from tools.arinc429_codec import ArincWord, ParityMode

if TYPE_CHECKING:
    from tools.pico429.client import Pico429Client


@dataclass
class BitVectorResult:
    index: int
    label_octal: int
    raw_tx: int
    raw_rx: int
    matched: bool
    rtt_ms: float
    description: str


@dataclass
class BitReport:
    tx_ch: int
    rx_ch: int
    high_speed: bool
    vectors: list[BitVectorResult]
    passed: bool
    error_message: str | None = None


# 与下位机固件及 Rust CLI 100% 对齐的 8 组基准向量
BIT_VECTORS: list[tuple[int, int, int, int, int, str]] = [
    (0o310, 0, 0x01234, 3, 0xE048D013, "高度基准字 (Altitude)"),
    (0o203, 1, 0x00567, 3, 0x60159DC1, "校正空速字 (Airspeed)"),
    (0o001, 2, 0x00001, 3, 0x60000680, "最小数值字 (Min Word)"),
    (0o377, 3, 0x7FFFF, 3, 0x7FFFFFFF, "全 1 满幅字 (All Ones)"),
    (0o156, 0, 0x00000, 0, 0x00000076, "全 0 极小子 (All Zeros)"),
    (0o114, 1, 0x55555, 3, 0xF5555532, "0x55 棋盘字 (Checkerboard 1)"),
    (0o220, 2, 0x2AAAA, 3, 0xEAAAAA09, "0xAA 棋盘字 (Checkerboard 2)"),
    (0o045, 3, 0x13579, 1, 0xA4D5E7A4, "混合工程字 (Mixed Engineering)"),
]


class BitEngine:
    """自动化自测试编排引擎 (纯业务逻辑，无 print 耦合)。"""

    @classmethod
    def run_bit(
        cls,
        client: Pico429Client,
        tx_ch: int = 0,
        rx_ch: int = 1,
    ) -> BitReport:
        # 1. 严格检查当前运行状态与硬件通道方向 (零隐式改动)
        try:
            cur_mode, cur_channels = client.query_status()
        except (OSError, RuntimeError, TimeoutError, ValueError) as e:
            return BitReport(
                tx_ch=tx_ch,
                rx_ch=rx_ch,
                high_speed=True,
                vectors=[],
                passed=False,
                error_message=f"查询初始状态失败: {e}",
            )

        if cur_mode == 0:
            return BitReport(
                tx_ch=tx_ch,
                rx_ch=rx_ch,
                high_speed=True,
                vectors=[],
                passed=False,
                error_message=(
                    "设备当前处于 CONFIG 静默维护模式，严禁执行自测试。\n"
                    "请先通过 'pico429 config mode run' 切换至运行模式，"
                    "或使用 'pico429 bit --profile <file>' 显式预装载测试配置。"
                ),
            )

        if not (0 <= tx_ch < len(cur_channels)) or not (0 <= rx_ch < len(cur_channels)):
            return BitReport(
                tx_ch=tx_ch,
                rx_ch=rx_ch,
                high_speed=True,
                vectors=[],
                passed=False,
                error_message=f"通道编号超限 (0..11): TX CH{tx_ch}, RX CH{rx_ch}",
            )

        tx_en, tx_is_rx, tx_hs = cur_channels[tx_ch]
        rx_en, rx_is_rx, rx_hs = cur_channels[rx_ch]

        if not tx_en or tx_is_rx:
            return BitReport(
                tx_ch=tx_ch,
                rx_ch=rx_ch,
                high_speed=tx_hs,
                vectors=[],
                passed=False,
                error_message=(
                    f"CH{tx_ch} 当前未配置为 TX 发送通道 (状态: en={tx_en}, dir={'RX' if tx_is_rx else 'TX'})。\n"
                    f"请先通过 'pico429 config channel {tx_ch} --dir tx' 配置，或装载配置文件。"
                ),
            )

        if not rx_en or not rx_is_rx:
            return BitReport(
                tx_ch=tx_ch,
                rx_ch=rx_ch,
                high_speed=rx_hs,
                vectors=[],
                passed=False,
                error_message=(
                    f"CH{rx_ch} 当前未配置为 RX 接收通道 (状态: en={rx_en}, dir={'RX' if rx_is_rx else 'TX'})。\n"
                    f"请先通过 'pico429 config channel {rx_ch} --dir rx' 配置，或装载配置文件。"
                ),
            )

        if tx_hs != rx_hs:
            return BitReport(
                tx_ch=tx_ch,
                rx_ch=rx_ch,
                high_speed=tx_hs,
                vectors=[],
                passed=False,
                error_message=(
                    f"发送端 CH{tx_ch} 速率 ({'100K' if tx_hs else '12.5K'}) 与接收端 CH{rx_ch} 速率 ({'100K' if rx_hs else '12.5K'}) 不匹配！"
                ),
            )

        # 2. 顺序回环验证 8 组测试向量 (硬件状态完全保持)
        vector_results: list[BitVectorResult] = []
        all_matched = True
        for i, (lbl, sdi, data, ssm, expected_raw, desc) in enumerate(BIT_VECTORS, 1):
            word = ArincWord.from_fields(lbl, sdi, data, ssm, ParityMode.ODD)
            if word.raw != expected_raw:
                all_matched = False
                vector_results.append(
                    BitVectorResult(
                        index=i,
                        label_octal=lbl,
                        raw_tx=word.raw,
                        raw_rx=0,
                        matched=False,
                        rtt_ms=0.0,
                        description=f"{desc} (本地向量编码错误)",
                    )
                )
                continue

            t0 = time.perf_counter()
            try:
                _, rx_frame = client.send_word_and_await_loopback(
                    tx_ch=tx_ch,
                    rx_ch=rx_ch,
                    raw_word=word.raw,
                    timeout=1.0,
                )
                rtt = (time.perf_counter() - t0) * 1000.0
                matched = rx_frame.payload_1 == expected_raw
                if not matched:
                    all_matched = False
                vector_results.append(
                    BitVectorResult(
                        index=i,
                        label_octal=lbl,
                        raw_tx=word.raw,
                        raw_rx=rx_frame.payload_1,
                        matched=matched,
                        rtt_ms=rtt,
                        description=desc,
                    )
                )
            except TimeoutError as err:
                all_matched = False
                vector_results.append(
                    BitVectorResult(
                        index=i,
                        label_octal=lbl,
                        raw_tx=word.raw,
                        raw_rx=0,
                        matched=False,
                        rtt_ms=0.0,
                        description=f"{desc} (超时未捕获回环: {err})",
                    )
                )

        return BitReport(
            tx_ch=tx_ch,
            rx_ch=rx_ch,
            high_speed=tx_hs,
            vectors=vector_results,
            passed=all_matched,
        )
