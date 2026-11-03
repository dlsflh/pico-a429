//! CYW43439 Wi-Fi 遥测与控制服务
//!
//! 在 Pico 2 W 上启用无线传输能力：
//! - 支持 UDP 广播 (端口 4290)：上位机无需配对，直接无线抓包
//! - 支持无线远程注入与配置

use embassy_net::udp::{PacketMetadata, UdpSocket};
use embassy_net::Stack;
use embassy_time::Timer;

use arinc429_core::protocol::BINARY_FRAME_LEN;

use crate::channel::GLOBAL_RX_PUBSUB;

pub const ARINC_UDP_PORT: u16 = 4290;

#[embassy_executor::task]
pub async fn net_task(mut runner: embassy_net::Runner<'static, cyw43::NetDriver<'static>>) -> ! {
    runner.run().await
}

#[embassy_executor::task]
pub async fn wifi_telemetry_task(stack: Stack<'static>) -> ! {
    let mut rx_meta = [PacketMetadata::EMPTY; 8];
    let mut rx_buffer = [0u8; 256];
    let mut tx_meta = [PacketMetadata::EMPTY; 8];
    let mut tx_buffer = [0u8; 256];

    let mut buf = [0u8; BINARY_FRAME_LEN];

    // 等待网络栈 Link Up 并分配 IP
    while !stack.is_link_up() {
        Timer::after_millis(200).await;
    }

    let mut socket = UdpSocket::new(
        stack,
        &mut rx_meta,
        &mut rx_buffer,
        &mut tx_meta,
        &mut tx_buffer,
    );

    // 绑定本地端口 4290
    if socket.bind(ARINC_UDP_PORT).is_err() {
        loop {
            Timer::after_secs(1).await;
        }
    }

    let broadcast_addr = embassy_net::IpEndpoint::new(
        embassy_net::IpAddress::v4(255, 255, 255, 255),
        ARINC_UDP_PORT,
    );

    let mut subscriber = GLOBAL_RX_PUBSUB
        .subscriber()
        .expect("Failed to acquire WiFi subscriber on GLOBAL_RX_PUBSUB");

    loop {
        // 从全局 ARINC 接收广播通道订阅数据 (多订阅者并行消费，绝不互斥丢帧)
        let frame = subscriber.next_message_pure().await;

        let bin = frame.to_binary_frame();
        bin.encode(&mut buf);

        // 通过 UDP 广播出去
        let _ = socket.send_to(&buf, broadcast_addr).await;
    }
}
