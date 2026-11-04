# Pico 2 12通道全量可配置 ARINC 429 系统演进路线图 (Roadmap)

本文档定义了将系统从当前双通道（CH0 TX, CH1 RX 固定方向）平滑升级至 **12 路物理通道全量独立动态可配置（任意通道均可独立软件配置为 TX/RX、100k/12.5k，免烧录在线热重构）** 的工业级技术方案与实施规划。

---

## 一、 系统架构与设计目标

* **通道容量**：12 路全双工/单工物理通道，任意通道在运行时均可独立配置为发送（TX）或接收（RX）。
* **速率支持**：各通道独立支持 High Speed (100 kbps) 与 Low Speed (12.5 kbps)，硬件物理斜率（Slew Rate）同步自适应。
* **硬件平台**：Raspberry Pi Pico 2（RP2350A，30 个可用 GPIO），充分榨干芯片的 3 块 PIO（12 个状态机）与 16 个 DMA 通道。
* **零指令重载开销**：单 PIO 块 32 指令空间同时常驻 TX (11 指令) 与 RX (20 指令)，总计占用 31/32 条指令，消除微码热重载的内存碎裂风险。
* **上位机协议**：USB 1.1 CDC ACM 免驱动高速数据流，定长 16 字节二进制帧聚合，单端点 64 字节零填充对齐。

---

## 二、 硬件架构与模拟前端 (AFE) 设计

ARINC 429 规范物理层为 $\pm 10\text{V}$ 双极性归零码 (BPRZ)，RP2350 的 3.3V TTL GPIO 无法直接驱动或接收高压总线，必须配合外围模拟前端扩展板实现通道方向复用。

### 1. 单通道电气拓扑（收发复用引脚设计）

对外为标准 ARINC 429 差分双绞线接插件（`ARINC_A`, `ARINC_B`）：
* **发送支路**：
  - 选用具备高阻关断特性的差分线路驱动芯片（如 **Holt HI-8592 / HI-8597**，或标准 HI-8585 后级串联固态信号继电器）；
  - 由微控制器引脚 `TX_A`、`TX_B` 输入 3.3V 逻辑信号；
  - 由慢速控制总线提供独立通道使能 `TX_EN`（高有效发送，低有效输出呈高阻态 Hi-Z，阻抗 $> 100\text{ k}\Omega$）与物理斜率控制 `SLR`（高电平 100k 高速斜率 1.5µs，低电平 12.5k 低速斜率 10µs）。
* **接收支路**：
  - 选用高输入阻抗差分线路接收芯片（如 **Holt HI-8448 / HI-8588**），输入阻抗 $> 12\text{ k}\Omega$，常态挂接在差分总线上；
  - 接收芯片输出 3.3V TTL 信号 `RX_A`、`RX_B`，**各串联一颗 $2.2\text{ k}\Omega$ 隔离保护电阻（或 74LVC1G125 三态缓冲器）** 后，与发送支路的 `TX_A`、`TX_B` 并联连接至同一对微控制器 GPIO。
* **复用电气机理**：
  - **TX 模式**：MCU GPIO 设为推挽输出。慢速总线置 `TX_EN = 1`。MCU 强驱动 `TX_A/B`，驱动芯片升压输出 $\pm 10\text{V}$。接收芯片输出端因 $2.2\text{ k}\Omega$ 隔离电阻被 MCU 低阻输出完全钳位吸收，无电气损伤；
  - **RX 模式**：MCU GPIO 设为高阻浮空输入（弱下拉）。慢速总线置 `TX_EN = 0`，驱动芯片输出呈现高阻断开。接收芯片经过 $2.2\text{ k}\Omega$ 电阻将解码后的 TTL 脉冲无损灌入 MCU PIO 输入端。

```text
               +-----------------------------------------------------------+
               |                    ARINC 429 Bus (A, B)                   |
               +-----------------------------+-----------------------------+
                                             |
                   +-------------------------+-------------------------+
                   |                                                   |
                   v (Differential high-Z in)                          | (Differential out)
           +---------------+                                   +---------------+
           | Line Receiver |                                   |  Line Driver  |
           |  (HI-8448)    |                                   |  (HI-8592)    |
           +-------+-------+                                   +-------+-------+
                   |                                                   ^
             RX_A  |  RX_B                                       TX_A  |  TX_B
                   v                                                   |
             [ 2.2k Resistors ]                                        |
                   |                                                   |
                   +-------------------------+-------------------------+
                                             | (Shared Pin Pair)
                                             v
                           +-----------------------------------+
                           | RP2350 GPIO Pin Pair [GP2n, GP2n+1]|
                           +-----------------------------------+
                                             ^
                                             | (Configuration)
                           +-----------------------------------+
                           | 74HC595 / PCA9555: TX_EN, SLR     |
                           +-----------------------------------+
```

### 2. Pico 2 (RP2350A) 引脚分配预算 (26 GPIO)

RP2350A 在标准 Pico 2 封装上引出 30 个可用 GPIO，按以下方案分配：
1. **24 个高速双向差分数据引脚（12 对相邻引脚）**：
   * 通道 0: `GP0, GP1` (PIO0 SM0)
   * 通道 1: `GP2, GP3` (PIO0 SM1)
   * 通道 2: `GP4, GP5` (PIO0 SM2)
   * 通道 3: `GP6, GP7` (PIO0 SM3)
   * 通道 4: `GP8, GP9` (PIO1 SM0)
   * 通道 5: `GP10, GP11` (PIO1 SM1)
   * 通道 6: `GP12, GP13` (PIO1 SM2)
   * 通道 7: `GP14, GP15` (PIO1 SM3)
   * 通道 8: `GP16, GP17` (PIO2 SM0)
   * 通道 9: `GP18, GP19` (PIO2 SM1)
   * 通道 10: `GP20, GP21` (PIO2 SM2)
   * 通道 11: `GP26, GP27` (PIO2 SM3)
2. **2 个慢速模拟前端控制引脚**：
   * `GP22`: 控制总线时钟/SCL (连接 3 片级联 74HC595 或 2 片 PCA9555)
   * `GP28`: 控制总线数据/SDA
   * 负责输出 12 路 `TX_EN`（高阻切换）与 12 路 `SLR`（高低速斜率）。
3. **USB 通信**：
   * 走 RP2350 原生 USB 控制器专用物理引脚（`USB_DP`, `USB_DM`），不消耗任何通用 GPIO。
4. **剩余引脚**：
   * `GP23, GP24, GP25, GP29` 保持空闲（或预留给板载状态 LED、同步触发外触发引脚）。

---

## 三、 主控与微架构资源核算

### 1. PIO 指令空间与统一时钟预算（单块 32 指令同时常驻 TX + RX）
* **TX 微码** (`arinc429_tx_program`): 占用 **11 条指令**，统一运行于 $1\text{ bit} = 20\text{ PIO cycles}$（100k 时 2 MHz，12.5k 时 250 kHz，严格 50.0% 占空比）。
* **RX 微码** (`arinc429_rx_program`): 占用 **20 条指令**，统一运行于 $1\text{ bit} = 20\text{ PIO cycles}$（100k 时 2 MHz，12.5k 时 250 kHz，带 3.4-bit 自愈间隙检测与脉冲中段二次采样）。
* **单块指令总和**：$11 + 20 = 31 \le 32$ 条指令！
* **时钟域统一红利**：
  - TX 与 RX 共享同一分频算法 `calculate_pio_clkdiv`；
  - 通道在线动态切换 TX/RX 时，**时钟分频比完全无需重新计算与刷新**，仅需重定向程序指针；
  - 在 RP2350 标准 150 MHz 主频下，100k 分频比为纯整数 75.0，12.5k 分频比为纯整数 600.0，彻底消除小数分频调制抖动；
  - 彻底告别运行时 `free_instr` / `load_program` 热重载带来的指令内存碎裂与时序不确定性！

### 2. DMA 通道 1:1 动态复用
* RP2350 拥有 16 个完全独立的物理 DMA 通道。
* 12 个物理通道各持 **1 个 DMA 通道**：
  - 处于 TX 模式时，该通道执行非阻塞 `sm.tx().dma_push(...)`；
  - 处于 RX 模式时，同一通道执行非阻塞 `sm.rx().dma_pull(...)`；
* 12 个通道消耗 12 个 DMA，**剩余 4 个 DMA 通道**可自由分配给 USB CDC 数据发送与系统后台调度。

---

## 四、 固件核心重构设计

### 1. 统一通道驱动 `Arinc429Channel`
消除原先分离的 `Arinc429Tx` 和 `Arinc429Rx` 结构体，将其融合为单一的通道实体：

```rust
pub struct Arinc429Channel<'d, T: Instance, const SM: usize> {
    sm: StateMachine<'d, T, SM>,
    dma: Channel<'d>,
    pin_a: Pin<'d, T>,
    pin_b: Pin<'d, T>,
    mode: ChannelDirection,
    speed: ArincSpeed,
    filter: LabelFilter,
    dma_buf: [u32; 1],
}

impl<'d, T: Instance, const SM: usize> Arinc429Channel<'d, T, SM> {
    /// 在线动态重新配置通道方向与波特率 (免烧录热重构)
    pub fn reconfigure(
        &mut self,
        progs: &SharedPioPrograms<'d, T>,
        new_mode: ChannelDirection,
        new_speed: ArincSpeed,
        afe: &mut AnalogFrontEndController,
    ) {
        // 1. 停机并彻底排空硬件残留 FIFO 与状态寄存器
        self.sm.set_enable(false);
        self.sm.clear_fifos();
        self.sm.restart();

        self.mode = new_mode;
        self.speed = new_speed;

        let sys_clk = clk_sys_freq();
        let mut cfg = Config::default();

        match new_mode {
            ChannelDirection::Tx => {
                // AFE 切换：置 TX_EN = 1 (开启驱动), 切换 SLR 物理斜率
                afe.set_channel_tx_enable(SM, true);
                afe.set_channel_slew_rate(SM, new_speed == ArincSpeed::High100k);

                // GPIO 切换：无上下拉，推挽输出
                self.pin_a.set_pull(Pull::None);
                self.pin_b.set_pull(Pull::None);
                self.sm.set_pin_dirs(Direction::Out, &[&self.pin_a, &self.pin_b]);

                cfg.use_program(&progs.tx_prog, &[]);
                cfg.set_set_pins(&[&self.pin_a, &self.pin_b]);
                cfg.set_out_pins(&[&self.pin_a, &self.pin_b]);
                cfg.shift_out = ShiftConfig {
                    auto_fill: false,
                    direction: ShiftDirection::Right,
                    threshold: 32,
                };
                cfg.clock_divider = calculate_pio_clkdiv(sys_clk, new_speed);
            }
            ChannelDirection::Rx => {
                // AFE 切换：置 TX_EN = 0 (高阻断开驱动)
                afe.set_channel_tx_enable(SM, false);
                afe.set_channel_slew_rate(SM, new_speed == ArincSpeed::High100k);

                // GPIO 切换：弱下拉，高阻输入
                self.pin_a.set_pull(Pull::Down);
                self.pin_b.set_pull(Pull::Down);
                self.sm.set_pin_dirs(Direction::In, &[&self.pin_a, &self.pin_b]);

                cfg.use_program(&progs.rx_prog, &[]);
                cfg.set_in_pins(&[&self.pin_a, &self.pin_b]);
                cfg.shift_in = ShiftConfig {
                    auto_fill: true, // 开启满 32 位硬件 Autopush
                    direction: ShiftDirection::Right,
                    threshold: 32,
                };
                cfg.shift_out = ShiftConfig {
                    auto_fill: false,
                    direction: ShiftDirection::Right,
                    threshold: 32,
                };
                cfg.clock_divider = calculate_pio_clkdiv(sys_clk, new_speed);
            }
            ChannelDirection::Disabled => {
                afe.set_channel_tx_enable(SM, false);
                self.pin_a.set_pull(Pull::None);
                self.pin_b.set_pull(Pull::None);
                self.sm.set_pin_dirs(Direction::In, &[&self.pin_a, &self.pin_b]);
                return;
            }
        }

        // 2. 装载新配置并重启状态机
        self.sm.set_config(&cfg);
        self.sm.set_enable(true);
    }
}
```

---

## 五、 实施路线图 (Milestones)

### Phase 1: 微代码与协议层基石固化 (已完成)
- [x] 完成 20 指令自愈 RX 微码（20 周期/bit）与 11 指令 TX 微码；
- [x] 实现独立四舍五入的 `calculate_tx_clkdiv` 与 `calculate_rx_clkdiv`；
- [x] 上位机 CLI 兼容 16 字节二进制 `CMD_ACK` 与流式大包解码器；
- [x] USB CDC 64 字节端点零长度包 (ZLP) 补齐。

### Phase 2: 外围模拟前端硬件扩展板制作 (待硬件投板)
- [ ] 绘制基于 RP2350 的 12 通道 ARINC 429 扩展子卡原理图；
- [ ] 选型：12 颗 HI-8597（或 6 颗双通道驱动芯片，带三态关断）+ 12 颗 HI-8448（四通道接收芯片）；
- [ ] 选型：3 片 74HC595 级联驱动 12 路 `TX_EN` 与 12 路 `SLR` 控制线；
- [ ] PCB 布线：严格保证 24 路高速差分信号等长走线与高频地平面隔离。

### Phase 3: 固件 12 通道矩阵重构 (软件迭代)
- [ ] 扩展 `channel.rs`，将通道数组扩展为 12 路独立发送队列与独立控制队列；
- [ ] 引入 `AnalogFrontEndController`，封装对 74HC595 / PCA9555 的异步移位刷新；
- [ ] 在 `main.rs` 中派发 12 个动态通道调度异步任务（采用 Embassy 任务池 `#[embassy_executor::task(pool_size = 12)]`）；
- [ ] 实现全通道矩阵式内建自测试 (BIT)，支持任意 $M$ 发送通道到任意 $N$ 接收通道的回环诊断。

### Phase 4: 上位机 CLI 多通道分析仪增强
- [ ] 支持按通道色彩标注多路并发捕获（Channel 0~11 彩色流式表格展示）；
- [ ] 增加 `arinc429-cli matrix-bit` 全通道交叉体检命令；
- [ ] 增加 PCAP / CSV 航空总线日志持久化转储与重放功能。
