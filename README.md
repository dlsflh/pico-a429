# pico-429: 基于 Pico 2 W (RP2350) 纯 PIO 异步驱动的 ARINC 429 通信系统

本项目使用 **Rust** 嵌入式生态，充分利用 **Raspberry Pi Pico 2 (RP2350)** 的 **PIO (可编程 I/O 状态机)** 与 **Embassy 异步硬件中断**，实现高精度、工业级鲁棒性的 **ARINC 429** 航空电子总线发送与接收系统。支持最大 12 通道物理扩展与硬件环回自收自发内建自测试 (BIT) 功能。

> 💡 **注**：Pico 2 W 上的 CYW43439 Wi-Fi 无线遥测能力规划请参阅演进路线图：[plan/todo/WIFI_ROADMAP.md](plan/todo/WIFI_ROADMAP.md)。当前固件聚焦于微秒级 USB CDC-ACM 与零阻塞 PIO 硬件链路。

---

## 一、 核心架构特性

1. **纳秒级时序与硬件级字间隙**：
   - 采用 PIO 状态机微代码实现差分双极性归零制（BPRZ）波形生成与采样，CPU 零开销；
   - 4-bit 物理字间隙由 PIO 微代码硬件固化（80 周期），零软件抖动；
   - 支持 **100 kbps (高速)** 与 **12.5 kbps (低速)** 模式，通过动态调整 PIO 时钟分频比寄存器（`clkdiv`）实现秒级重配；
2. **零 DMA 纯硬件 FIFO 异步驱动**：
   - 彻底移除非必要 DMA 通道依赖，杜绝 128 字节硬对齐陷阱与 Errata RP2350-E5 竞态；
   - 依靠 PIO 4 字深硬件 FIFO（提供 1.44 ms 硬件防抖）与中断唤醒 `.await`，12 通道满载仅消耗 < 1.5% CPU；
3. **微码常驻共存（31/32 字空间）**：
   - TX 微码（11 条）+ RX 微码（20 条）= 31 条指令，单块 PIO 内存（32 字）一次性常驻装载，通道切换收发方向零动态重载开销；
4. **$O(1)$ 高性能 Label 过滤器**：
   - 内置 256-bit 位图过滤引擎（仅占 32 字节 RAM），支持白名单、黑名单与全放行模式；
5. **双上位机工具链与防死锁 USB CDC**：
   - USB CDC IN 写入增加 100ms 超时保护，RX 入队非阻塞，彻底消除 Windows `Write timeout`；
   - 提供**免编译 Python 极速工具链 (`tools/pico429.py`)** 与 **Rust 官方 CLI (`arinc429-host-cli`)**。

---

## 二、 硬件引脚分配 (默认单卡 Channel 0 TX / Channel 1 RX)

| 物理引脚 | GPIO 编号 | 信号名称 | 信号方向 | 连接外部芯片引脚 | 功能说明 |
| :---: | :---: | :---: | :---: | :---: | :--- |
| **Pin 1** | **GP0** | `TX_A` | 输出 (MCU $\to$ Driver) | HI-8585 `TXINA` | 逻辑 1 时输出前半周正脉冲 |
| **Pin 2** | **GP1** | `TX_B` | 输出 (MCU $\to$ Driver) | HI-8585 `TXINB` | 逻辑 0 时输出前半周正脉冲 |
| **Pin 3** | **GND** | 地线 | - | 驱动板 GND | 必须共地 |
| **Pin 4** | **GP2** | `RX_A` | 输入 (Receiver $\to$ MCU) | HI-8588 `ROUTA` | 总线为 $+10\text{V}$ 时输出高电平 |
| **Pin 5** | **GP3** | `RX_B` | 输入 (Receiver $\to$ MCU) | HI-8588 `ROUTB` | 总线为 $-10\text{V}$ 时输出高电平 |
| **Pin 36**| **3V3** | 电源 | 输出 (3.3V) | 接收芯片 VCC | 可供电给 3.3V 接收比较器 |

> **自测短接环回**：在无收发芯片时，可直接用杜邦线短接 `GP0 -> GP2` 与 `GP1 -> GP3` 进行硬件 BIT 环回自测。

---

## 三、 工程目录结构

```
pico-429/
├── Cargo.toml                 # 工作空间根配置 (含 Dev/Release Profile 优化与 LTO)
├── .cargo/config.toml         # 目标 target: thumbv8m.main-none-eabihf 与链接脚本配置
├── tools/
│   └── pico429.py             # 免编译 Python 极速工具 (支持 ping, status, send, listen, bit)
├── docs/
│   ├── HARDWARE_MANUAL.md     # 硬件连线详细手册 (芯片引脚、分立电路、环回自测)
│   └── USER_GUIDE.md          # 软件使用手册 (固件烧录、host-cli、串口命令、排错)
├── crates/
│   ├── arinc429-core/         # 纯 no_std 核心协议库 (32位字格式、奇偶校验、位图过滤)
│   ├── firmware/              # Pico 2 固件 (Embassy 异步协程、PIO 微代码、USB CDC)
│   └── host-cli/              # PC 上位机命令行调试与分析工具 (支持抓包与 BIT 环回)
```

---

## 四、 快速上手指南

### 1. 运行核心协议栈单元测试
```bash
cargo test -p arinc429-core
cargo test -p arinc429-host-cli
```

### 2. 编译与生成 UF2 固件
```bash
cargo build --package pico-429-firmware --target thumbv8m.main-none-eabihf --release
picotool uf2 convert target/thumbv8m.main-none-eabihf/release/pico-429-firmware pico429.uf2
```
按住 Pico 2 W 上的 **BOOTSEL** 按钮插入 USB，将 `pico429.uf2` 拖入弹出的 U 盘盘符完成烧录。

### 3. 上位机交互与自检

#### 方式 A：Python 极速工具（推荐，免编译，自动发现串口）
```bash
# 1. 握手连通性测试 (PING -> PONG)
uv run --with pyserial python tools/pico429.py ping

# 2. 查询通道状态
uv run --with pyserial python tools/pico429.py status

# 3. 执行硬件自收自发 BIT 闭环自测试 (GP0->GP2, GP1->GP3)
uv run --with pyserial python tools/pico429.py bit

# 4. 发送单个构造字
uv run --with pyserial python tools/pico429.py send --label 310 --data 0x1234

# 5. 彩色监听总线数据流
uv run --with pyserial python tools/pico429.py listen
```

#### 方式 B：Rust 官方 CLI
```bash
cargo run --package arinc429-host-cli -- status --port COM3
cargo run --package arinc429-host-cli -- bit --port COM3 --tx 0 --rx 1
cargo run --package arinc429-host-cli -- listen --port COM3
```
