//! ARINC 429 内建自测试 (BIT - Built-In-Test) 核心算法与测试向量 (no_std)
//!
//! 提供闭环自收自发、故障注入与物理层健全性诊断：
//! 1. Walking 1s / Walking 0s 位走查 (排除总线 Stuck-at-0 / Stuck-at-1 故障)
//! 2. 经典航电 Label 校验与 LSB 逆序闭环一致性
//! 3. 奇偶校验翻转错误注入 (Negative Test) 与异常捕获率验证
//! 4. 回路往返时延 (RTT) 窗口评估

use crate::parity::{apply_parity, ParityMode};
use crate::word::ArincWord;

/// 自测试执行状态
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum BitStatus {
    Passed,
    Failed,
}

/// 自测试详细错误诊断代码
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum BitErrorCode {
    None = 0,
    Timeout = 1,
    DataMismatch = 2,
    ParityInjectionNotDetected = 3,
    InvalidFrameReceived = 4,
    LatencyAnomaly = 5,
}

/// 自测试综合诊断报告
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct BitReport {
    pub tx_channel: u8,
    pub rx_channel: u8,
    pub status: BitStatus,
    pub error_code: BitErrorCode,
    pub words_sent: u16,
    pub words_received: u16,
    pub parity_injections_tested: u16,
    pub parity_errors_caught: u16,
    pub min_latency_us: u32,
    pub max_latency_us: u32,
    pub avg_latency_us: u32,
}

/// 单个测试向量描述
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BitVector {
    /// 待发送的原始 32 位字
    pub raw_word: u32,
    /// 该测试向量是否故意注入了校验错误 (true 表示非法帧，应当被接收端捕获标记)
    pub expect_parity_error: bool,
}

/// 生成全套确定性 BIT 测试向量 (约 70 个测试字)
///
/// 包含：
/// 1. 32 个 Walking 1s (验证从 Bit 1 到 Bit 32 无 Stuck-at-0)
/// 2. 32 个 Walking 0s (验证从 Bit 1 到 Bit 32 无 Stuck-at-1)
/// 3. 3 个标准航电 Label 工程字
/// 4. 2 个故意注入的奇偶校验损坏字 (Even Parity)
pub fn generate_bit_vectors<const N: usize>() -> heapless::Vec<BitVector, N> {
    let mut list = heapless::Vec::new();

    // 1. Walking 1s (第 0 位到第 30 位分别置 1，第 31 位由奇校验计算保证)
    for bit in 0..31 {
        let raw = 1u32 << bit;
        let with_parity = apply_parity(raw, ParityMode::Odd);
        let _ = list.push(BitVector {
            raw_word: with_parity,
            expect_parity_error: false,
        });
    }

    // 2. Walking 0s (31 位全 1 中仅 1 位为 0)
    for bit in 0..31 {
        let raw = !(1u32 << bit) & 0x7FFF_FFFF;
        let with_parity = apply_parity(raw, ParityMode::Odd);
        let _ = list.push(BitVector {
            raw_word: with_parity,
            expect_parity_error: false,
        });
    }

    // 3. 标准工程航电参数 (Label 0310 俯仰角，Label 0203 气压高，Label 0377 设备标识)
    let words = [
        ArincWord::builder().octal_label(0o310).sdi(1).data(0x12345).ssm(3).build(),
        ArincWord::builder().octal_label(0o203).sdi(2).data(0x0ABCD).ssm(0).build(),
        ArincWord::builder().octal_label(0o377).sdi(0).data(0x7FFFF).ssm(3).build(),
    ];
    for w in words {
        let _ = list.push(BitVector {
            raw_word: w.to_raw(),
            expect_parity_error: false,
        });
    }

    // 4. 故障注入：故意注入 Even Parity (偶校验在奇校验系统中必为非法帧)
    let corrupt1 = apply_parity(0x0001_2345, ParityMode::Even);
    let corrupt2 = apply_parity(0x7654_3210, ParityMode::Even);
    let _ = list.push(BitVector {
        raw_word: corrupt1,
        expect_parity_error: true,
    });
    let _ = list.push(BitVector {
        raw_word: corrupt2,
        expect_parity_error: true,
    });

    list
}

/// 接收到的自检结果项
#[derive(Debug, Clone, Copy)]
pub struct BitReceivedItem {
    pub raw_word: u32,
    pub parity_ok: bool,
    pub latency_us: u32,
}

/// 校验评估接收端捕获的所有 BIT 数据帧
pub fn evaluate_bit_results(
    tx_ch: u8,
    rx_ch: u8,
    sent: &[BitVector],
    received: &[BitReceivedItem],
) -> BitReport {
    let words_sent = sent.len() as u16;
    let words_received = received.len() as u16;

    let mut report = BitReport {
        tx_channel: tx_ch,
        rx_channel: rx_ch,
        status: BitStatus::Failed,
        error_code: BitErrorCode::None,
        words_sent,
        words_received,
        parity_injections_tested: 0,
        parity_errors_caught: 0,
        min_latency_us: u32::MAX,
        max_latency_us: 0,
        avg_latency_us: 0,
    };

    if words_received < words_sent {
        report.error_code = BitErrorCode::Timeout;
        return report;
    }

    let mut total_latency: u64 = 0;

    for (i, vec) in sent.iter().enumerate() {
        let rx_item = received[i];

        // 校验字内容匹配
        if rx_item.raw_word != vec.raw_word {
            report.error_code = BitErrorCode::DataMismatch;
            return report;
        }

        // 延迟统计
        if rx_item.latency_us < report.min_latency_us {
            report.min_latency_us = rx_item.latency_us;
        }
        if rx_item.latency_us > report.max_latency_us {
            report.max_latency_us = rx_item.latency_us;
        }
        total_latency += rx_item.latency_us as u64;

        // 校验错误捕获评估
        if vec.expect_parity_error {
            report.parity_injections_tested += 1;
            if !rx_item.parity_ok {
                report.parity_errors_caught += 1;
            }
        }
    }

    // 奇校验注入未被捕获检测
    if report.parity_injections_tested > 0
        && report.parity_errors_caught != report.parity_injections_tested
    {
        report.error_code = BitErrorCode::ParityInjectionNotDetected;
        return report;
    }

    if words_received > 0 {
        report.avg_latency_us = (total_latency / words_received as u64) as u32;
    } else {
        report.min_latency_us = 0;
    }

    report.status = BitStatus::Passed;
    report.error_code = BitErrorCode::None;
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_and_evaluate_bit_vectors_success() {
        let vectors = generate_bit_vectors::<128>();
        assert_eq!(vectors.len(), 31 + 31 + 3 + 2); // 67 vectors

        // 模拟完美回环接收
        let mut simulated_rx = heapless::Vec::<BitReceivedItem, 128>::new();
        for vec in &vectors {
            let _ = simulated_rx.push(BitReceivedItem {
                raw_word: vec.raw_word,
                parity_ok: !vec.expect_parity_error,
                latency_us: 360,
            });
        }

        let report = evaluate_bit_results(0, 1, &vectors, &simulated_rx);
        assert_eq!(report.status, BitStatus::Passed);
        assert_eq!(report.error_code, BitErrorCode::None);
        assert_eq!(report.words_sent, 67);
        assert_eq!(report.words_received, 67);
        assert_eq!(report.parity_injections_tested, 2);
        assert_eq!(report.parity_errors_caught, 2);
        assert_eq!(report.avg_latency_us, 360);
    }

    #[test]
    fn test_evaluate_bit_vectors_data_mismatch() {
        let vectors = generate_bit_vectors::<128>();
        let mut simulated_rx = heapless::Vec::<BitReceivedItem, 128>::new();
        for (i, vec) in vectors.iter().enumerate() {
            let raw = if i == 5 { vec.raw_word ^ 0x01 } else { vec.raw_word };
            let _ = simulated_rx.push(BitReceivedItem {
                raw_word: raw,
                parity_ok: !vec.expect_parity_error,
                latency_us: 360,
            });
        }

        let report = evaluate_bit_results(0, 1, &vectors, &simulated_rx);
        assert_eq!(report.status, BitStatus::Failed);
        assert_eq!(report.error_code, BitErrorCode::DataMismatch);
    }

    #[test]
    fn test_evaluate_bit_vectors_timeout() {
        let vectors = generate_bit_vectors::<128>();
        let mut simulated_rx = heapless::Vec::<BitReceivedItem, 128>::new();
        // 仅收到 10 帧
        for vec in vectors.iter().take(10) {
            let _ = simulated_rx.push(BitReceivedItem {
                raw_word: vec.raw_word,
                parity_ok: !vec.expect_parity_error,
                latency_us: 360,
            });
        }

        let report = evaluate_bit_results(0, 1, &vectors, &simulated_rx);
        assert_eq!(report.status, BitStatus::Failed);
        assert_eq!(report.error_code, BitErrorCode::Timeout);
    }
}
