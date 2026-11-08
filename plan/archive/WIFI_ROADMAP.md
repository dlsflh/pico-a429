# Pico 2 W Wi-Fi 遥测与无线接入演进路线图 (Roadmap - 已撤销并归档)

> [!WARNING]
> **状态变更 (2026-11)**：经系统物理层吞吐与硬件资源冲突审查，**本项目已正式撤销 Wi-Fi 无线遥测计划并予以全案归档**。系统架构永久聚焦于有线 USB 极速确定性传输与 12 通道物理全覆盖。

---

## 一、 背景与原设计目标 (历史记录)

* **应用场景**：机载/台架现场免接线排故、便携式平板/PC 局域网总线监听、机舱内无线数据抓包。
* **通信协议**：UDP 组播/局域网广播 (默认端口 `4290`)，传输定长 16 字节标准二进制遥测帧（`BinaryFrame`）。
* **吞吐与延迟指标**：单通道 100 kbps 满载流量约 3.1 kpps (~44 KB/s)，Wi-Fi 需保证丢包率 $< 0.1\%$，端到端延迟 $< 5\text{ ms}$。

---

## 二、 硬件资源与潜在冲突分析 (历史记录)

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

## 三、 架构设计与实现步骤 (历史方案备忘)

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

## 四、 路线图评估结论与正式归档撤销说明 (2026-11 架构审查结论)

> [!IMPORTANT]
> **审查结论**：**正式撤销 Wi-Fi 无线遥测扩展计划，全案永久归档并自待办规划清单（todo）移除**。

### 撤销技术依据与物理事实：
1. **MCU 与 Wi-Fi 芯片物理接口瓶颈限制，无代际速度优势**：
   - Pico 2 W 上的 CYW43439 仅通过 **1-bit 半双工 gSPI (PIO 模拟，GPIO 24)** 与 RP2350 互联，时钟上限仅 ~30 MHz。
   - 考虑半双工总线换向、包头封装、协议栈校验与 ACK 交互，实际应用层净有效吞吐仅 **8 ~ 15 Mbps (约 1.0 ~ 1.8 MB/s)**，与 Full-Speed USB (12 Mbps, 实际 ~1.0 MB/s) 处于同一量级。Wi-Fi 在该硬件架构下没有任何速度优势。
2. **与 12 通道物理全覆盖目标硬性冲突（硬件资源互斥）**：
   - RP2350 全片仅有 12 个 PIO 状态机（PIO0, PIO1, PIO2 各 4 个）。
   - 12 通道独立动态可配置（TX/RX）刚好耗尽全部 12 个状态机。
   - 启用 `cyw43-pio` 必须霸占 1 个状态机，导致 ARINC 429 通道数被永久腰斩为 11 通道，与工程核心目标不可调和。
3. **高吞吐航电总线实时确定性恶化**：
   - 12 通道 100 kbps 满载流量达到 33,333 fps (533 KB/s)。
   - 2.4 GHz 无线信道存在 50~200 ms 随机争抢抖动与 1%~5% 的工业电磁环境丢包率；UDP 丢包不可靠，TCP 重传等待则瞬间引发 MCU 内存耗尽。
4. **系统架构减负**：
   - 彻底解除未来对 CYW43 专有闭源固件 Blob、`embassy-net` 协议栈、热点连接管理等重型代码的维护包袱，专注于微秒级硬实时 USB 有线采集。
