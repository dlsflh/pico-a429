use arinc429_core::protocol::BinaryFrame;
use pico429::telemetry::{CurrentValueTable, TelemetryRingBuffers};
use pico429::transport::scheduler::{ChannelSpeed, StreamScheduler};
use std::sync::Arc;

#[test]
fn test_scheduler_dynamic_credit_ceiling_and_watermarks() {
    let scheduler = StreamScheduler::new();

    // Default HighSpeed (100k)
    let ch0 = scheduler.channel(0);
    let (credit, sent, consumed) = ch0.snapshot();
    assert_eq!(credit, 128);
    assert_eq!(sent, 0);
    assert_eq!(consumed, 0);

    // Switch Channel 1 to LowSpeed (12.5k)
    let ch1 = scheduler.channel(1);
    ch1.set_speed(ChannelSpeed::Low12k5);
    let (credit1, _, _) = ch1.snapshot();
    assert_eq!(credit1, 32);
}

#[tokio::test]
async fn test_relative_baseline_latching_on_reconnect() {
    let scheduler = StreamScheduler::new();
    let ch = scheduler.channel(2);

    // Simulate firmware already transmitted 50,000 words prior to host start
    ch.on_credit_update(128, 50_000);

    // First snapshot should latch baseline 50,000, so in-flight is 0 and usable credit is 128
    let (credit, sent, consumed) = ch.snapshot();
    assert_eq!(credit, 128);
    assert_eq!(sent, 0);
    assert_eq!(consumed, 50_000);

    // Acquire credit for 10 words
    for _ in 0..10 {
        ch.acquire_credit().await.expect("Credit available");
    }

    let (credit_after_send, sent_after_send, _) = ch.snapshot();
    assert_eq!(sent_after_send, 10);
    assert_eq!(credit_after_send, 118);

    // Firmware reports 50,008 words consumed
    ch.on_credit_update(126, 50_008);
    let (credit_refill, _, _) = ch.snapshot();
    // In-flight is now 10 - 8 = 2, so usable credit is 128 - 2 = 126
    assert_eq!(credit_refill, 126);
}

#[test]
fn test_scheduler_abort_resets_in_flight() {
    let scheduler = StreamScheduler::new();
    let ch = scheduler.channel(3);

    ch.on_credit_update(128, 1000);
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        for _ in 0..50 {
            ch.acquire_credit().await.unwrap();
        }
    });

    let (credit_before, sent_before, _) = ch.snapshot();
    assert_eq!(sent_before, 50);
    assert_eq!(credit_before, 78);

    // Abort
    ch.abort();
    let (credit_after, sent_after, _) = ch.snapshot();
    assert_eq!(sent_after, 0);
    assert_eq!(credit_after, 128);
}

#[tokio::test]
async fn test_multichannel_independent_concurrency() {
    let scheduler = Arc::new(StreamScheduler::new());

    // Starve Channel 0 (available_slots = 0)
    scheduler.channel(0).on_credit_update(0, 0);

    // Channel 1 has full credits
    scheduler.channel(1).on_credit_update(128, 0);

    // Acquiring credit on Channel 1 must NOT be blocked by Channel 0
    let res = tokio::time::timeout(std::time::Duration::from_millis(50), async {
        scheduler.channel(1).acquire_credit().await
    })
    .await;

    assert!(
        res.is_ok(),
        "Channel 1 should acquire credit immediately without cross-channel blocking"
    );
}

#[test]
fn test_current_value_table_o1_lookup() {
    let cvt = CurrentValueTable::new();

    // Create ARINC word: Label 0o310, data 0x12345
    let raw = arinc429_core::word::encode_word(0o310, 0x12345, 1, 2, "odd").unwrap();
    let frame = BinaryFrame::new_full(1, 0, 0, 100, 500_000, raw);

    cvt.update_from_frame(&frame);

    let val = cvt
        .get_label(0, 0o310)
        .expect("Label 0o310 should exist in CVT");
    assert_eq!(val.channel, 0);
    assert_eq!(val.label_octal, 0o310);
    assert_eq!(val.label_str, "310");
    assert_eq!(val.raw_word, raw);
    assert_eq!(val.data, 0x12345);
    assert_eq!(val.sdi, 1);
    assert_eq!(val.ssm, 2);
    assert!(val.parity_ok);
    assert_eq!(val.timestamp_us, 500_000);

    // Unreceived label returns None
    assert!(cvt.get_label(0, 0o270).is_none());
}

#[test]
fn test_telemetry_ring_buffer_overwrites() {
    let buffers = TelemetryRingBuffers::new(10);

    for i in 0..25 {
        let frame = BinaryFrame::new_full(1, 0, 0, i as u16, i * 100, 0x12345678);
        buffers.push(frame);
    }

    let stats = buffers.stats(0);
    assert_eq!(stats.total_received, 25);
    assert_eq!(stats.dropped_overwrites, 15);

    let fetched = buffers.fetch(0, 10, None);
    assert_eq!(fetched.len(), 10);
    // Most recent frame should be seq 24
    assert_eq!(fetched[0].seq_num, 24);
}
