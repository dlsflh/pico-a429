# pico-429: 基于 Pico 2 W (RP2350) PIO 的 ARINC 429 航电通信系统

本项目使用 **Rust** 嵌入式生态，充分利用 **Raspberry Pi Pico 2 (RP2350)** 的 **PIO (可编程 I/O 状态机)** 与 **DMA** 能力，实现了高精度、工业级鲁棒性的 **ARINC 429** 航空电子总线发送与接收系统。同时支持最大 12 通道收发扩展与自收自发内建自测试 (BIT) 功能。

> 💡 **注**：Pico 2 W 上的 CYW43439 Wi-Fi 无线遥测能力规划请参阅演进路线图：[plan/todo/WIFI_ROADMAP.md](plan/todo/WIFI_ROADMAP.md)。生产分支当前完全聚焦于高确定性微秒级 USB CDC-ACM 与 DMA 硬件链路。

---

## 一、 核心特性与架构亮点

1. **纳秒级精确时序与高低速动态切换**：
   - 采用 PIO 状态机微代码实现差分双极性归零制（BPRZ）波形生成与采样，CPU 零开销；
   - 支持 **100 kbps (高速)** 与 **12.5 kbps (低速)** 模式，通过动态调整 PIO 时钟分频器寄存器（`clkdiv`）实现**一键热切换**，无时钟累积误差；
2. **硬件级抗滑移自同步与自动校验**：
   - 接收状态机集成 **字间隙 (Inter-word Gap) 超时检测**：无论设备在何时接入总线，均能在检测到 $\ge 2$ 个位周期的空闲后强制对齐新帧起点，杜绝位滑移（Bit Slip）；
   - **自动奇偶校验**：发送端支持自动计算填充 ARINC 429 标准奇校验（Odd Parity）；接收端单指令完成校验，支持校验错误帧自动丢弃或上报；
3. **$O(1)$ 高性能 Label 过滤器**：
   - 内置 256-bit 位图过滤引擎（仅占 32 字节 RAM），支持白名单、黑名单与全放行模式；
   - 支持 SDI 掩码级联过滤，确保上位机不被总线无关高频心跳淹没；
4. **高性能上位机接口 (全速 USB CDC ACM)**：
   - **USB 虚拟串口 (12 Mbps)**：免驱即插即用，支持定长 14 字节高速二进制帧（微秒时间戳 + CRC16 校验和）与 ASCII 终端文本双向交互；
   - **自收自发内建自测试 (BIT)**：软硬件闭环自检，自动执行 Walking 1s/0s、奇偶校验反转与时延基准诊断；
5. **深度极致优化的编译选项**：
   - 发布构建全面开启 **Fat LTO (全程序链接时优化)** 与 `panic = "abort"`，固件 Flash 大小极致收敛至 **$39.4\text{ KB}$**；
   - 开发构建开启外部依赖项预优化缓存 (`[profile.dev.package."*"] opt-level = 2`)，增量构建仅需 **$1.5$ 秒**；
6. **模块化与多通道扩展性**：
   - RP2350 具备 3 个 PIO 块、12 个状态机与 16 个 DMA 通道，软件采用统一的消息总线设计，可平滑扩展至多路收发。

---

## 二、 硬件引脚分配一览 (Channel 0)

> **重要提示**：Pico 2 W GPIO 为 3.3V CMOS 电平，ARINC 429 差分总线为 $\pm10\text{V}$，**严禁直连总线**。必须通过收发芯片（如 Holt HI-8585 / HI-8588）或前级电平转换电路。

| 物理引脚 | GPIO 编号 | 信号名称 | 信号方向 | 连接外部芯片引脚 | 功能说明 |
| :---: | :---: | :---: | :---: | :---: | :--- |
| **Pin 1** | **GP0** | `TX_A` | 输出 (MCU $\to$ Driver) | HI-8585 `TXINA` | 逻辑 1 时输出前半周正脉冲 |
| **Pin 2** | **GP1** | `TX_B` | 输出 (MCU $\to$ Driver) | HI-8585 `TXINB` | 逻辑 0 时输出前半周正脉冲 |
| **Pin 3** | **GND** | 地线 | - | 驱动板 GND | 必须共地 |
| **Pin 4** | **GP2** | `RX_A` | 输入 (Receiver $\to$ MCU) | HI-8588 `ROUTA` | 总线为 $+10\text{V}$ 时输出高电平 |
| **Pin 5** | **GP3** | `RX_B` | 输入 (Receiver $\to$ MCU) | HI-8588 `ROUTB` | 总线为 $-10\text{V}$ 时输出高电平 |
| **Pin 36**| **3V3** | 电源 | 输出 (3.3V) | 接收芯片 VCC | 可供电给 3.3V 接收比较器 |

> 📖 **详细硬件连接图、收发芯片原理图与免芯片自测试验台接线**，请参阅：
> 👉 **[硬件连线与电气设计手册 (docs/HARDWARE_MANUAL.md)](docs/HARDWARE_MANUAL.md)**

---

## 三、 工程目录结构

```
pico-429/
├── Cargo.toml                 # 工作空间根配置 (含 Dev/Release Profile 优化与 LTO)
├── .cargo/config.toml         # 目标 target: thumbv8m.main-none-eabihf 与链接脚本配置
├── docs/
│   ├── HARDWARE_MANUAL.md     # 硬件连线详细手册 (芯片引脚、分立电路、环回自测)
│   └── USER_GUIDE.md          # 软件使用手册 (固件烧录、host-cli、串口命令、排错)
├── crates/
│   ├── arinc429-core/         # 纯 no_std 核心协议库 (32位字格式、奇偶校验、位图过滤)
│   ├── firmware/              # Pico 2 固件 (Embassy 执行器、PIO 汇编、DMA 引擎、USB CDC)
│   └── host-cli/              # PC 上位机命令行调试与分析工具 (支持串口抓包与 BIT 自检)
```

---

## 四、 快速上手指南

### 1. 运行核心协议栈单元测试
```bash
cargo test -p arinc429-core
```
覆盖全部 16 项关键测试：包括奇偶校验、八进制与 LSB 逆序转换、256 位位图过滤、BNR/BCD 编解码与二进制帧 CRC16 验证。

### 2. 编译 Pico 2 W 固件
```bash
cargo build --package pico-429-firmware --target thumbv8m.main-none-eabihf --release
```
编译产物位于 `target/thumbv8m.main-none-eabihf/release/pico-429-firmware`（仅 39.4 KB）。

### 3. 使用 PC 上位机工具 (`host-cli`) 进行监控
```bash
# 监听 USB 串口并以彩色表格实时展示 429 数据
cargo run --package arinc429-host-cli -- listen --port COM3

# 执行自收自发 BIT 自检 (通道 0 发送 -> 通道 1 接收)
cargo run --package arinc429-host-cli -- bit --port COM3 --tx 0 --rx 1

# 发送构造字 (Label: 0310, SDI: 1, Data: 0x1234, SSM: 3)
cargo run --package arinc429-host-cli -- send --port COM3 --label 310 --sdi 1 --data 0x1234 --ssm 3

# 切换为 12.5k 低速模式
cargo run --package arinc429-host-cli -- speed --port COM3 --channel 0 --speed 12.5k

# 白名单放行指定 Label
cargo run --package arinc429-host-cli -- filter --port COM3 --channel 0 --allow 310
```

> 📖 **关于普通串口工具（PuTTY/SSCOM）交互命令与详细使用说明**，请参阅：
> 👉 **[软件使用与上位机通信手册 (docs/USER_GUIDE.md)](docs/USER_GUIDE.md)**
