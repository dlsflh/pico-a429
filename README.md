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
5. **免编译 Python 极速工具链与防死锁 USB CDC**：
   - USB CDC IN 写入增加 100ms 超时保护，RX 入队非阻塞，彻底消除 Windows `Write timeout`；
   - 提供**免编译 Python 极速工具链 (`tools/pico429.py`)**，支持 12 通道一键 TOML 载入、状态导出、片上 Flash 配置持久化与交互自检。

---

## 二、 硬件引脚分配 (12 通道物理引脚分布 GP0..GP21, GP26..GP27)

| 通道 | 状态机分配 | 物理引脚 A (Pin A) | 物理引脚 B (Pin B) | 出厂默认方向 | 默认速率 |
| :---: | :---: | :---: | :---: | :---: | :---: |
| **CH0** | `PIO0_SM0` | **GP0** (Pin 1) | **GP1** (Pin 2) | TX (发送) | High (100k) |
| **CH1** | `PIO0_SM1` | **GP2** (Pin 4) | **GP3** (Pin 5) | RX (接收) | High (100k) |
| **CH2** | `PIO0_SM2` | **GP4** (Pin 6) | **GP5** (Pin 7) | RX (接收) | High (100k) |
| **CH3** | `PIO0_SM3` | **GP6** (Pin 9) | **GP7** (Pin 10) | RX (接收) | High (100k) |
| **CH4** | `PIO1_SM0` | **GP8** (Pin 11) | **GP9** (Pin 12) | RX (接收) | High (100k) |
| **CH5** | `PIO1_SM1` | **GP10** (Pin 14) | **GP11** (Pin 15) | RX (接收) | High (100k) |
| **CH6** | `PIO1_SM2` | **GP12** (Pin 16) | **GP13** (Pin 17) | RX (接收) | High (100k) |
| **CH7** | `PIO1_SM3` | **GP14** (Pin 19) | **GP15** (Pin 20) | RX (接收) | High (100k) |
| **CH8** | `PIO2_SM0` | **GP16** (Pin 21) | **GP17** (Pin 22) | RX (接收) | High (100k) |
| **CH9** | `PIO2_SM1` | **GP18** (Pin 24) | **GP19** (Pin 25) | RX (接收) | High (100k) |
| **CH10** | `PIO2_SM2` | **GP20** (Pin 26) | **GP21** (Pin 27) | RX (接收) | High (100k) |
| **CH11** | `PIO2_SM3` | **GP26** (Pin 31) | **GP27** (Pin 32) | RX (接收) | High (100k) |

> **出厂默认安全状态**：CH0 为 TX, CH1 为 RX（保持 GP0..GP3 双通道测试夹具兼容），CH2..11 为被动监听 RX（高阻弱下拉，电气绝对安全）。
> **引脚说明**：Pico 物理 Pin 30 为硬件 `RUN` (复位脚)，GP23 为板载内部专用引脚；因此 Channel 11 采用排针引出的 `GP26` (Pin 31) 与 `GP27` (Pin 32)。
> **自测短接环回**：在无外部收发芯片时，可直接用杜邦线短接 `GP0 -> GP2` 与 `GP1 -> GP3` 执行单卡闭环 BIT。

---

## 三、 工程目录结构

```
pico-429/
├── Cargo.toml                 # 工作空间根配置 (含 Dev/Release Profile 优化与 LTO)
├── .cargo/config.toml         # 目标 target: thumbv8m.main-none-eabihf 与链接脚本配置
├── tools/
│   ├── pico429.py             # 免编译 Python 极速工具 (支持 profile, interactive, bit, config, speed, filter 等)
│   └── arinc429_codec.py      # 纯算法协议编解码库 (支持字打包、奇偶校验、CRC16-CCITT)
├── tests/
│   ├── test_codec.py          # Python 协议与基准 BIT 向量单元测试
│   └── test_profile_toml.py   # Python 12 通道配置方案与 TOML 编解码行为测试
├── docs/
│   ├── HARDWARE_MANUAL.md     # 硬件连线详细手册 (芯片引脚、分立电路、环回自测)
│   └── USER_GUIDE.md          # 软件使用手册 (固件烧录、串口命令、TOML 配置、排错)
└── crates/
    ├── arinc429-core/         # 纯 no_std 核心协议库 (32位字格式、奇偶校验、位图过滤、控制命令解析)
    └── firmware/              # Pico 2 固件 (Embassy 异步协程、PIO 微代码、片上 Flash、USB CDC)
```

---

## 四、 快速上手指南

### 1. 运行核心单元测试
```bash
# 1. 运行 Rust 协议核心测试
cargo test -p arinc429-core --no-default-features

# 2. 运行 Python 上位机协议与 TOML 配置测试
uv run python -m unittest discover tests
```

### 2. 编译与生成 UF2 固件
```bash
cargo build --package pico-429-firmware --target thumbv8m.main-none-eabihf --release
# 注意：Cargo 产物无扩展名，picotool 需显式通过 -t elf 指定文件类型
picotool uf2 convert target/thumbv8m.main-none-eabihf/release/pico-429-firmware -t elf pico429.uf2
```
按住 Pico 2 W 上的 **BOOTSEL** 按钮插入 USB，将 `pico429.uf2` 拖入弹出的 U 盘盘符完成烧录。

### 3. Python 极速工具使用指南 (推荐，免编译，自动识别串口)
```bash
# 1. 握手连通性测试 (PING -> PONG，RTT < 0.5ms)
uv run --with pyserial python tools/pico429.py ping

# 2. 查询 12 通道在线状态
uv run --with pyserial python tools/pico429.py status

# 3. 硬件自收自发 BIT 闭环自测试 (GP0->GP2, GP1->GP3，RTT < 1ms)
uv run --with pyserial python tools/pico429.py bit

# 4. 用户自驱动实时回环终端 (开启常驻会话，输入载荷即时回显)
uv run --with pyserial python tools/pico429.py interactive

# 5. 动态单通道启停与速率切换 (速率支持 high / low)
uv run --with pyserial python tools/pico429.py config 0 --enable --dir TX --speed high
uv run --with pyserial python tools/pico429.py speed 0 low
uv run --with pyserial python tools/pico429.py filter 1 --allow 310

# 6. 12 通道 TOML 配置文件管理与一键加载 (模式 B)
# 导出当前固件 12 通道配置为 TOML 模板
uv run --with pyserial python tools/pico429.py profile dump -o my_bench.toml

# 按照 TOML 配置文件一键原子重构所有 12 通道 (安全暂停 -> 应用 -> 恢复)
uv run --with pyserial python tools/pico429.py profile apply my_bench.toml

# 将当前配置持久化保存至片上 Flash 存储槽位 (0..3)
uv run --with pyserial python tools/pico429.py profile save 0 --name "Avionics_Bench"

# 列出片上 Flash 中的所有配置方案与开机自启动槽位
uv run --with pyserial python tools/pico429.py profile list

# 切换激活指定的片上 Flash 槽位
uv run --with pyserial python tools/pico429.py profile load 0

# 设置指定槽位为上电默认自启动
uv run --with pyserial python tools/pico429.py profile boot 0

# 7. 发送单个构造字
uv run --with pyserial python tools/pico429.py send --label 310 --data 0x1234

# 8. 彩色监听总线数据流
uv run --with pyserial python tools/pico429.py listen
```
