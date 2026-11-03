# Pico 2 W Wi-Fi 遥测与无线接入演进路线图 (Roadmap)

本文档记录了将 Raspberry Pi Pico 2 W (RP2350) 上的 CYW43439 Wi-Fi 芯片接入 ARINC 429 航电通信系统的技术规划与设计规范。在当前的精简与确定性实时架构中，Wi-Fi 相关代码已从生产分支彻底移除，待后续时机成熟时按本路线图重启。

---

## 一、 背景与设计目标

* **应用场景**：机载/台架现场免接线排故、便携式平板/PC 局域网总线监听、机舱内无线数据抓包。
* **通信协议**：UDP 组播/局域网广播 (默认端口 `4290`)，传输定长 14 字节标准二进制遥测帧（`BinaryFrame`）。
* **吞吐与延迟指标**：单通道 100 kbps 满载流量约 3.1 kpps (~44 KB/s)，Wi-Fi 需保证丢包率 $< 0.1\%$，端到端延迟 $< 5\text{ ms}$。

---

## 二、 硬件资源与潜在冲突分析

在 Pico 2 W 上启用 Wi-Fi 需要占用专用引脚与硬件外设：
1. **CYW43 专用引脚**：
   * `WL_REG_ON` (GPIO 23): 芯片电源供电使能
   * `DATA / SPI_DIO` (GPIO 24): 半双工数据总线
   * `CS` (GPIO 25): 片选
   * `CLK` (GPIO 29): 时钟
2. **PIO 资源占用**：
   * `cyw43-pio` 需要独占 1 个 PIO 状态机（通常占用 PIO1 或 PIO2）。
   * 需特别注意：ARINC 429 多通道系统最多使用 12 个状态机（跨 PIO0, PIO1, PIO2）。若启用 Wi-Fi，最大 ARINC 429 通道数将缩减至 11 通道。
3. **RAM 占用估算**：
   * CYW43 固件固化微代码（Firmware Blob）与驱动状态机需消耗约 20 KB ~ 32 KB RAM。
   * `embassy-net` 协议栈（TCP/IP/UDP Socket 缓冲区）需消耗约 16 KB RAM。
   * RP2350 拥有 512 KB SRAM，在内存预算上完全充裕。

---

## 三、 架构设计与实现步骤

### 1. 固件固化与依赖配置
```toml
# crates/firmware/Cargo.toml
cyw43 = { version = "0.7.0", features = ["defmt", "firmware-logs"] }
cyw43-pio = { version = "0.10.0", features = ["defmt"] }
embassy-net = { version = "0.7.0", features = ["defmt", "udp", "dhcpv4", "medium-ethernet"] }
```
需在构建阶段或 Flash 特定区域预烧录 `cyw43439` 的固件与 CLM blob。

### 2. 独立网络调度任务
```rust
#[embassy_executor::task]
pub async fn net_task(mut runner: embassy_net::Runner<'static, cyw43::NetDriver<'static>>) -> ! {
    runner.run().await
}

#[embassy_executor::task]
pub async fn wifi_telemetry_task(
    stack: Stack<'static>,
    mut subscriber: embassy_sync::pubsub::Subscriber<'static, ...>
) -> ! {
    // 监听 GLOBAL_RX_PUBSUB 订阅端并封装 UDP 发送
}
```

### 3. AP / STA 双模支持
* **默认 SoftAP 模式**：Pico 2 W 自建热点 `Pico429-XXXX`，密码 `arinc429wifi`，上位机直连；
* **STA 客户端模式**：通过串口命令 `WIFICONNECT <SSID> <PASSWORD>` 接入现场已有无线局域网。

---

## 四、 激活前置验收条件
1. 12 通道 ARINC 429 DMA 架构成熟稳定且主控时钟无抖动；
2. PIO 块明确划分出 1 个 SM 供给 `cyw43-pio`；
3. 上位机 `host-cli` 增补完备的 UDP 抓包缓冲与防掉包重排序算法。
