//! 固件端 BIT (内建自测试) 执行与协调调度引擎
//!
//! 在两通道硬件短接（或测试台回路）时，执行端到端自收自发与故障注入诊断：
//! - 依次下发 Walking 1s/0s、工程字与偶校验注入帧
//! - 微秒级高精度评估回路时延 RTT
//! - 通过 BIT_RESP_SIGNAL 向上位机报告结构化体检结果

use embassy_futures::select::{select, Either};
use embassy_time::{Duration, Instant, Timer};
use heapless::Vec;

use arinc429_core::bit::{
    evaluate_bit_results, generate_bit_vectors, BitErrorCode, BitReceivedItem, BitReport, BitStatus,
};

use crate::channel::{
    BitRequest, OutboundArincFrame, BIT_REQ_CHANNEL, BIT_RESP_SIGNAL, GLOBAL_RX_PUBSUB,
    MAX_CHANNELS,
};

#[embassy_executor::task]
pub async fn bit_service_task() {
    let mut subscriber = GLOBAL_RX_PUBSUB
        .subscriber()
        .expect("Failed to acquire BIT subscriber on GLOBAL_RX_PUBSUB");

    loop {
        // 等待上位机触发 BIT 自检请求
        let req = BIT_REQ_CHANNEL.receive().await;
        let report = run_bit_cycle(req, &mut subscriber).await;
        BIT_RESP_SIGNAL.signal(report);
    }
}

async fn run_bit_cycle(
    req: BitRequest,
    subscriber: &mut embassy_sync::pubsub::Subscriber<
        'static,
        embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex,
        crate::channel::InboundArincFrame,
        128,
        4,
        12,
    >,
) -> BitReport {
    let tx_ch = req.tx_channel;
    let rx_ch = req.rx_channel;

    // 参数范围检查
    if tx_ch as usize >= MAX_CHANNELS || rx_ch as usize >= MAX_CHANNELS {
        return BitReport {
            tx_channel: tx_ch,
            rx_channel: rx_ch,
            status: BitStatus::Failed,
            error_code: BitErrorCode::Timeout,
            words_sent: 0,
            words_received: 0,
            parity_injections_tested: 0,
            parity_errors_caught: 0,
            min_latency_us: 0,
            max_latency_us: 0,
            avg_latency_us: 0,
        };
    }

    // 清空历史订阅滞留消息
    while subscriber.try_next_message_pure().is_some() {}

    let vectors = generate_bit_vectors::<128>();
    let mut rx_items = Vec::<BitReceivedItem, 128>::new();

    for vec in &vectors {
        let t_start = Instant::now();

        // 投递发送帧
        let outbound = OutboundArincFrame {
            channel_id: tx_ch,
            raw_word: vec.raw_word,
        };
        crate::channel::CHANNEL_TX_QUEUES[tx_ch as usize]
            .send(outbound)
            .await;

        // 等待回环接收该帧 (设置 50ms 超时阈值，足够 100k 或 12.5k 传输)
        let deadline = Instant::now() + Duration::from_millis(50);
        let mut got_item = false;

        loop {
            let timeout_fut = Timer::at(deadline);
            match select(subscriber.next_message_pure(), timeout_fut).await {
                Either::First(frame) => {
                    if frame.channel_id == rx_ch {
                        let latency = (Instant::now() - t_start).as_micros() as u32;
                        let _ = rx_items.push(BitReceivedItem {
                            raw_word: frame.word.to_raw(),
                            parity_ok: frame.parity_ok,
                            latency_us: latency,
                        });
                        got_item = true;
                        break;
                    }
                }
                Either::Second(_) => {
                    // 超时未收到对应通道的回环帧
                    break;
                }
            }
        }

        if !got_item {
            break;
        }
    }

    evaluate_bit_results(tx_ch, rx_ch, &vectors, &rx_items)
}
