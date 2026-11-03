# ARINC 429 Pico 2 W 软件使用与上位机通信手册

本手册详细说明如何烧录 Pico 2 固件，以及如何通过 USB 虚拟串口与上位机进行双向通信、报文解析、BIT 自测试验、过滤设置与故障排查。

> 💡 **Wi-Fi 路线图**：Wi-Fi 无线遥测相关演进规划请参阅 `plan/todo/WIFI_ROADMAP.md`。

---

## 一、 固件烧录与启动

### 方法 A：通过 SWD 硬件调试器一键烧录 (推荐)
如果手头有 Raspberry Pi Debug Probe 或兼容 CMSIS-DAP 调试器，将 SWD 引脚（SWDIO、SWCLK、GND）连接至 Pico 2 W 调试口：
```bash
# 自动编译并在芯片上运行固件并打开 RTT 日志
probe-rs run --chip RP2350 target/thumbv8m.main-none-eabihf/release/pico-429-firmware
```

### 方法 B：通过 BOOTSEL 按钮拖拽 UF2 文件烧录
若无调试器，可生成标准 `.uf2` 固件：
1. 安装转换工具：
   ```bash
   cargo install elf2uf2-rs
   ```
2. 生成 UF2 镜像：
   ```bash
   elf2uf2-rs target/thumbv8m.main-none-eabihf/release/pico-429-firmware pico429.uf2
   ```
3. 按住 Pico 2 W 上的 **BOOTSEL 按钮** 不放，将 Micro-USB / Type-C 数据线插入电脑。电脑将识别为一个名为 `RP2350` 的可移动磁盘；
4. 将 `pico429.uf2` 拖入该磁盘，芯片自动重启并启动固件。

---

## 二、 模式一：使用官方上位机分析工具 (`arinc429-host-cli`)

本项目在 `crates/host-cli` 目录下提供了跨平台的 PC 终端分析仪工具，无需安装任何臃肿的第三方软件。

### 1. 实时监听 USB 串口并彩色解析 429 报文
通过 USB 连接 Pico 2 W：
```bash
# Windows (假设端口为 COM3)
cargo run --package arinc429-host-cli -- listen --port COM3

# Linux (通常为 /dev/ttyACM0)
cargo run --package arinc429-host-cli -- listen --port /dev/ttyACM0
```

**终端输出效果：**
```
================================================================================================
通道   时间戳(s)    速率   Label    SDI   Data         SSM   校验     RAW
------------------------------------------------------------------------------------------------
CH0    1.024510     100k   310      1     0x00012345   3     OK       0x812345C8
CH0    1.034502     100k   203      0     0x0000ABCD   0     OK       0x00ABCD83
CH0    1.044520     100k   310      1     0x00012345   3     FAIL     0x012345C8
```
- **Label**：自动完成 LSB 物理传输位翻转，直接显示为航电工程八进制（如 `310`）；
- **校验状态**：自动核验第 32 位奇校验，正确显示绿色 **`OK`**，损坏显示红色加粗 **`FAIL`**；
- **时间戳**：MCU 纳秒级硬件捕获时间（微秒精度），便于分析总线抖动与周期。

---

### 2. 执行自收自发 BIT (内建自测试)
当两组 ARINC 429 通道在硬件上短接形成回路（如 CH0 TX 与 CH1 RX，或免芯片环回自测 GP0-GP2, GP1-GP3）时，可通过上位机发起全自动硬件自检：
```bash
cargo run --package arinc429-host-cli -- bit --port COM3 --tx 0 --rx 1
```
固件将自动执行 100k/12.5k 速率切换、Walking 1s/0s 位走查、奇校验错误注入与回路往返时延（RTT）基准评估，并在终端打印结构化诊断报告。

---

### 3. 向总线发送一个构造的 ARINC 429 字
```bash
# 语法：send --port <PORT> --label <八进制Label> --sdi <SDI> --data <十六进制Data> --ssm <SSM>
cargo run --package arinc429-host-cli -- send --port COM3 --label 310 --sdi 1 --data 0x1234 --ssm 3
```
- 固件会自动根据 ARINC 429 规范计算奇校验位，并将第 32 位补齐后由 PIO 发射。

---

### 4. 远程一键热切换波特率 (高速 100k vs 低速 12.5k)
```bash
# 切换为 100 kbps (高速)
cargo run --package arinc429-host-cli -- speed --port COM3 --channel 0 --speed 100k

# 切换为 12.5 kbps (低速)
cargo run --package arinc429-host-cli -- speed --port COM3 --channel 0 --speed 12.5k
```

---

### 5. 动态配置 Label 过滤规则
```bash
# 白名单模式：仅放行 Label 310
cargo run --package arinc429-host-cli -- filter --port COM3 --channel 0 --allow 310

# 黑名单模式：屏蔽高频广播 Label 377
cargo run --package arinc429-host-cli -- filter --port COM3 --channel 0 --block 377

# 清除所有规则 (恢复全放行)
cargo run --package arinc429-host-cli -- filter --port COM3 --channel 0 --clear
```

---

## 三、 模式二：使用通用串口调试助手 (ASCII 交互终端模式)

若在没有 Rust 环境的现场电脑上，您可以使用任意串口调试工具（如 **PuTTY**, **MobaXterm**, **SecureCRT**, **SSCOM**, **XCOM**）打开虚拟串口（波特率设为 115200 即可）：

### 1. 切换为 ASCII 人类可读文本模式
向串口发送指令：
```text
MODE ASCII\r\n
```
此时固件将停止发送 14 字节二进制数据包，改为直接打印可读字符串：
```text
[CH0] T:1234567us 100K L:310 SDI:1 DATA:0x01234 SSM:3 P:OK (0x812345C8)
[CH0] T:1244560us 100K L:203 SDI:0 DATA:0x0ABCD SSM:0 P:OK (0x00ABCD83)
```

### 2. 串口控制指令字典汇总

| 指令格式 | 功能说明 | 示例 | 预期响应 |
| :--- | :--- | :--- | :--- |
| `STATUS` | 查询固件状态与当前模式 | `STATUS` | `STATUS: Pico-429 Online. Mode: ASCII` |
| `MODE <BIN\|ASCII>` | 切换数据上报格式 | `MODE BIN` | `OK: Switched to BINARY mode` |
| `SPEED <ch> <100K\|12K5>` | 切换指定通道波特率 | `SPEED 0 100K` | `OK: Channel 0 set to 100K` |
| `TX <ch> <oct> <sdi> <data> <ssm>` | 自动计算校验并发送 | `TX 0 310 1 0x1234 3` | `OK: Sent L:310 SDI:1 DATA:0x1234 RAW:0x812345C8` |
| `TXRAW <ch> <hex_word>` | 直接发射 32 位原始十六进制字 | `TXRAW 0 0x812345C8` | `OK: Queued raw word 0x812345C8 for CH0` |
| `FILTER <ch> ALLOW <oct>` | 白名单放行指定 Label | `FILTER 0 ALLOW 310` | `OK: Filter rule updated` |
| `FILTER <ch> BLOCK <oct>` | 黑名单屏蔽指定 Label | `FILTER 0 BLOCK 377` | `OK: Filter rule updated` |
| `FILTER <ch> CLEAR` | 清空过滤规则 | `FILTER 0 CLEAR` | `OK: Filter rule updated` |
| `BIT <tx_ch> <rx_ch>` | 触发两通道硬件闭环 BIT 诊断 | `BIT 0 1` | `OK: BIT CH0->CH1 PASSED [SENT:67 RECV:67 ...]` |

---

## 四、 常见问题与排错指引 (Troubleshooting)

### Q1: 串口收到大量 `P:ERR` (Parity Error)
- **原因 1：波特率不匹配**。对方总线是 12.5 kbps 低速，但 Pico 2 W 处于 100 kbps 高速模式（或反之）。执行 `SPEED 0 12K5` 或 `SPEED 0 100K` 切换；
- **原因 2：差分 A/B 极性接反**。将总线 Line A 与 Line B 调换接线；
- **原因 3：未共地**。外部收发电路的 GND 必须与 Pico 2 W 的 GND 引脚连接。

### Q2: 串口收不到任何数据
- 检查外部接收芯片供电（如 HI-8588 的 VCC 是否有 3.3V/5V）；
- 在没有外部芯片时，执行前文所述的 **限流电阻短接自测环回（GP0->GP2, GP1->GP3）**，若环回自测正常收到数据，则 100% 说明固件正常，故障在外部调理电路。

### Q3: 为什么 Label 0310 发送时填入的低字节和收到的十六进制不一样？
- ARINC 429 规范标准：Bit 1 先发，Bit 8 后发。八进制数值 0310（`0b11_001_000`）按位反转后为 `0b0001_0011`（`0x13`）。固件底层已全自动处理该位反转，上位机直接读写八进制即可。
