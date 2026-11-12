# pico-429: 基于 RP2350 (Pico 2) 纯 PIO 异步驱动的 12 通道 ARINC 429 通信系统

本项目使用 **纯 Rust** 生态，充分榨干 **Raspberry Pi Pico 2 (RP2350)** 的 **3 块硬件 PIO (全部 12 个可编程状态机)** 与 **Embassy 异步硬件中断**，实现高精度、工业级鲁棒性的 **12 通道 ARINC 429** 航空电子总线发送与接收系统。配套提供高性能纯 Rust 上位机引擎与原生 Model Context Protocol (MCP) 服务，支持 AI 编程助手直接驱动硬件自检与流式收发。

> 💡 **架构说明**：系统将 RP2350 全片 12 个 PIO 状态机 100% 独占分配给 12 路物理通道，专注于微秒级 USB CDC-ACM 确定性传输。Wi-Fi 方案因 1-bit gSPI 速度瓶颈与抢占 PIO 状态机冲突已被审查并正式归档（详见 [plan/archive/WIFI_ROADMAP.md](plan/archive/WIFI_ROADMAP.md)）。

---

## 一、 核心架构特性

1. **纳秒级时序与硬件级字间隙**：
   - 采用 PIO 状态机微代码实现差分双极性归零制（BPRZ）波形生成与采样，CPU 零开销；
   - 4-bit 物理字间隙由 PIO 微代码硬件固化（80 周期），零软件抖动；
   - 支持 **100 kbps (高速)** 与 **12.5 kbps (低速)** 模式，通过动态调整 PIO 时钟分频比寄存器（`clkdiv`）实现微秒级重配；
2. **零 DMA 纯硬件 FIFO 异步驱动**：
   - 彻底移除非必要 DMA 通道依赖，杜绝 128 字节硬对齐陷阱与 Errata RP2350-E5 竞态；
   - 依靠 PIO 4 字深硬件 FIFO（提供 1.44 ms 硬件防抖）与中断唤醒 `.await`，12 通道满载仅消耗 < 1.5% CPU；
3. **微码常驻共存（31/32 字空间）**：
   - TX 微码（11 条）+ RX 微码（20 条）= 31 条指令，单块 PIO 内存（32 字）一次性常驻装载，通道切换收发方向零动态重载开销；
4. **$O(1)$ 高性能 Label 过滤器**：
   - 内置 256-bit 位图过滤引擎（仅占 32 字节 RAM），支持白名单、黑名单与全放行模式；
5. **全对称 16 字节纯二进制协议与单核确定性执行**：
   - 全面淘汰 ASCII 文本指令，采用 16 字节定长二进制帧（`BinaryFrame`）及硬件 CRC16 校验；
   - 固件消除所有动态字符串格式化与堆内存分配，Flash 空间大幅缩减 42 KB，彻底避免 USB 阻塞与内存碎裂；
6. **纯 Rust 高吞吐上位机与原生 MCP 智能体服务 (`crates/pico429`)**：
   - 基于信用额度的闭环动态流控，支持高达 2,777 words/s（100 kbps 物理总线饱和速率）无丢包持续发流；
   - 12 通道 $\times$ 256 Label 零锁原子当前值表（CVT）支持 $O(1)$ 瞬时遥测索引；
   - 深度 32,768 环形历史缓冲与无缝覆盖；
   - 原生内置 Model Context Protocol (MCP) 服务器，支持 AI 编程助手（Claude Desktop、Cursor、VS Code MCP、Antigravity）一键无缝接入。

---

## 二、 硬件引脚分配 (12 通道物理引脚分布 GP0..GP21, GP26..GP27)

| 通道 | 状态机分配 | 物理引脚 A (Pin A) | 物理引脚 B (Pin B) | 上电初始基态 | 默认速率 |
| :---: | :---: | :---: | :---: | :---: | :---: |
| **CH0** | `PIO0_SM0` | **GP0** (Pin 1) | **GP1** (Pin 2) | Disabled (静默) | High (100k) |
| **CH1** | `PIO0_SM1` | **GP2** (Pin 4) | **GP3** (Pin 5) | Disabled (静默) | High (100k) |
| **CH2** | `PIO0_SM2` | **GP4** (Pin 6) | **GP5** (Pin 7) | Disabled (静默) | High (100k) |
| **CH3** | `PIO0_SM3` | **GP6** (Pin 9) | **GP7** (Pin 10) | Disabled (静默) | High (100k) |
| **CH4** | `PIO1_SM0` | **GP8** (Pin 11) | **GP9** (Pin 12) | Disabled (静默) | High (100k) |
| **CH5** | `PIO1_SM1` | **GP10** (Pin 14) | **GP11** (Pin 15) | Disabled (静默) | High (100k) |
| **CH6** | `PIO1_SM2` | **GP12** (Pin 16) | **GP13** (Pin 17) | Disabled (静默) | High (100k) |
| **CH7** | `PIO1_SM3` | **GP14** (Pin 19) | **GP15** (Pin 20) | Disabled (静默) | High (100k) |
| **CH8** | `PIO2_SM0` | **GP16** (Pin 21) | **GP17** (Pin 22) | Disabled (静默) | High (100k) |
| **CH9** | `PIO2_SM1` | **GP18** (Pin 24) | **GP19** (Pin 25) | Disabled (静默) | High (100k) |
| **CH10** | `PIO2_SM2` | **GP20** (Pin 26) | **GP21** (Pin 27) | Disabled (静默) | High (100k) |
| **CH11** | `PIO2_SM3` | **GP26** (Pin 31) | **GP27** (Pin 32) | Disabled (静默) | High (100k) |

> **出厂默认安全状态**：上电时固件默认进入 **Config 模式**，全部 12 通道处于 Disabled 静默状态（高阻弱下拉，电气绝对安全），等待上位机下发批量配置；若 Flash 槽位配置了开机自启动 Profile，则在上电时自动原子加载并切入 **Operational 模式**。
> **引脚说明**：Pico 物理 Pin 30 为硬件 `RUN` (复位脚)，GP23 为板载内部专用引脚；因此 Channel 11 采用排针引出的 `GP26` (Pin 31) 与 `GP27` (Pin 32)。
> **自测短接环回**：在无外部收发芯片时，可直接用杜邦线短接 `GP0 -> GP2` 与 `GP1 -> GP3` 执行单卡闭环 BIT。

---

## 三、 工程目录结构

```
pico-429/
├── Cargo.toml                 # 工作空间根配置 (多 Crate Monorepo 管理)
├── .cargo/config.toml         # 目标 target: thumbv8m.main-none-eabihf 与链接脚本配置
├── docs/
│   ├── HARDWARE_MANUAL.md     # 硬件连线详细手册 (芯片引脚、分立电路、环回自测)
│   ├── USER_GUIDE.md          # 软件使用手册 (固件烧录、串口命令、TOML 配置、MCP 集成)
│   ├── designs/               # 核心系统架构设计文档
│   └── reviews/               # 历史审查与质量审计归档
├── crates/
│   ├── arinc429-core/         # 纯 no_std 核心协议库 (32位字格式、奇偶校验、位图过滤、流传输协议)
│   ├── firmware/              # Pico 2 嵌入式固件 (Embassy 异步协程、PIO 微码、Flash、USB CDC)
│   └── pico429/               # 纯 Rust 上位机引擎、CLI 终端与原生 MCP 服务器
└── profiles/                  # 12 通道 TOML 声明式配置示例
```

---

## 四、 快速上手指南

### 1. 运行工作空间测试套件
```bash
# 运行核心协议库与流协议测试
cargo test --package arinc429-core

# 运行上位机调度器、CVT 与流控集成测试
cargo test --package pico429
```

### 2. 编译与生成 UF2 固件
```bash
# 1. 交叉编译 Release 固件
cargo build --package pico-429-firmware --target thumbv8m.main-none-eabihf --release

# 2. 转换为 Pico 2 识别的 UF2 格式
picotool uf2 convert target/thumbv8m.main-none-eabihf/release/pico-429-firmware -t elf pico429.uf2
```
按住 Pico 2 W 上的 **BOOTSEL** 按钮插入 USB，将 `pico429.uf2` 拖入弹出的 U 盘盘符完成烧录。

### 3. 安装与使用纯 Rust 上位机 (`pico429`)

推荐将 `pico429` 一键安装到全局环境：
```bash
# 安装至 ~/.cargo/bin (需确保在 PATH 中)
cargo install --path crates/pico429
```
安装后即可在任何终端免编译秒级调用；亦可使用 `cargo run --release --package pico429 -- <command>` 直接运行。

#### 常用交互命令：
```bash
# 1. 探测串口并查询 12 通道状态
pico429 status

# 2. 模式切换 (run 物理运行模式 / config 高阻维护模式)
pico429 config mode run
pico429 config mode config

# 3. 通道参数配置与 Flash Profile 固化
pico429 config channel 0 --dir tx --speed high
pico429 config channel 1 --dir rx --speed high

# 加载并固化 TOML 方案至 Flash 槽位 0
pico429 config profile apply profiles/loopback_ch0_ch1.toml --slot 0
# 查看 4 个 Flash 存储槽位清单
pico429 config profile list
# 设定开机默认自启动槽位
pico429 config profile boot 0

# 4. 硬件自收自发 BIT 闭环自测试 (CH0 TX -> CH1 RX)
pico429 bit --tx 0 --rx 1 --count 20

# 5. 单字发送 (自动计算物理奇偶校验位)
pico429 send --channel 0 --label 310 --data 0x1234 --sdi 1 --ssm 3 --parity odd

# 6. 高吞吐流式批量注入 (支持指定 32 位原始字与线速速率 high/low)
pico429 stream --channel 0 -n 10000 --speed high --data 0xE048D013

# 7. 实时总线遥测监听终端 (支持通道过滤与帧数限制)
pico429 listen --channel 1 -n 100
```

### 4. 启动原生 MCP 智能体服务器

```bash
pico429 mcp serve
```

在 Claude Desktop 或 Cursor 的 `mcpServers` 中配置：
```json
{
  "mcpServers": {
    "pico429": {
      "command": "pico429",
      "args": ["mcp", "serve"]
    }
  }
}
```
配置完成后，AI 助手将直接具备 9 项硬件级操作工具，可自主完成固件状态检测、参数配置、流式发流以及环回测试。
