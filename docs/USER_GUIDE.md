# ARINC 429 Pico 2 W 软件使用与上位机通信手册

本手册详细说明如何烧录 Pico 2 W 固件，以及如何通过 Python 极速脚本、Rust 官方 CLI 与串口调试助手进行双向通信、报文解析、BIT 自测与过滤配置。

---

## 一、 固件编译与烧录

### 1. 编译生成 UF2 镜像 (推荐)
```bash
cargo build --package pico-429-firmware --target thumbv8m.main-none-eabihf --release
picotool uf2 convert target/thumbv8m.main-none-eabihf/release/pico-429-firmware pico429.uf2
```

### 2. 烧录固件
1. 按住 Pico 2 W 上的 **BOOTSEL 按钮** 不放，插入 USB 线。电脑识别出 `RPI-RP2` 磁盘；
2. 将 `pico429.uf2` 拖入该磁盘，芯片自动重启进入运行状态。

---

## 二、 上位机工具使用指南

### 工具 A：Python 极速免编译工具 (`tools/pico429.py` - 推荐)
基于 `uv run --with pyserial`，无需配置 Python 环境，自动扫描匹配树莓派 Pico 串口：

```bash
# 1. 握手测试 (PING -> PONG，RTT < 0.5ms)
uv run --with pyserial python tools/pico429.py ping

# 2. 查询固件在线状态与通道配置
uv run --with pyserial python tools/pico429.py status

# 3. 硬件自收自发 BIT 闭环自测试 (GP0->GP2, GP1->GP3，RTT < 1ms)
uv run --with pyserial python tools/pico429.py bit

# 4. 用户自驱动实时回环终端 (开启常驻会话，输入 1234 即时回显并计算 RTT)
uv run --with pyserial python tools/pico429.py interactive

# 5. 动态配置通道启停、方向与速率 (CHCFG)
uv run --with pyserial python tools/pico429.py config 0 --enable --dir TX --speed 100k
uv run --with pyserial python tools/pico429.py config 1 --disable

# 6. 切换指定通道波特率
uv run --with pyserial python tools/pico429.py speed 0 100k

# 7. 配置 Label 硬件过滤规则 (白名单 / 黑名单 / 清空)
uv run --with pyserial python tools/pico429.py filter 1 --allow 310
uv run --with pyserial python tools/pico429.py filter 1 --clear

# 8. 切换上报模式 (BIN / ASCII)
uv run --with pyserial python tools/pico429.py mode ascii

# 9. 发送单个构造字 (Label 310, 数据 0x1234)
uv run --with pyserial python tools/pico429.py send --label 310 --data 0x1234

# 10. 彩色流式监听并解码接收数据
uv run --with pyserial python tools/pico429.py listen
```

---

### 工具 B：Rust 官方 CLI 分析工具 (`arinc429-host-cli`)
```bash
# 查询通道状态
cargo run --package arinc429-host-cli -- status --port COM3

# 硬件自收自发 BIT 自检
cargo run --package arinc429-host-cli -- bit --port COM3 --tx 0 --rx 1

# 实时监听 USB 串口
cargo run --package arinc429-host-cli -- listen --port COM3

# 发送构造字
cargo run --package arinc429-host-cli -- send --port COM3 --label 310 --sdi 1 --data 0x1234 --ssm 3

# 切换通道波特率
cargo run --package arinc429-host-cli -- speed --port COM3 --channel 0 --speed 12.5k

# 白名单放行 Label
cargo run --package arinc429-host-cli -- filter --port COM3 --channel 0 --allow 310
```

---

### 工具 C：通用串口助手 (PuTTY / SSCOM / XCOM 等)
串口波特率配置为 115200（固件默认 ASCII 交互）：

| 指令格式 | 功能说明 | 示例 | 预期响应 |
| :--- | :--- | :--- | :--- |
| `PING` | 极速连通性心跳 | `PING` | `PONG` |
| `STATUS [ch]` | 查询固件状态与当前通道方向 | `STATUS` | `STATUS: Pico-429 Online [CH0: Tx, CH1: Rx]. Mode: ASCII` |
| `MODE <BIN\|ASCII>` | 切换数据上报格式 | `MODE BIN` | `OK: Switched to BINARY mode` |
| `CHCFG <ch> SPEED <100K\|12K5>` | 切换指定通道波特率 | `CHCFG 0 SPEED 100K` | `OK: Channel 0 set to 100K` |
| `CHCFG <ch> FILTER ALLOW <oct>` | 白名单放行指定 Label | `CHCFG 1 FILTER ALLOW 310` | `OK: Filter rule Allowed on Channel 1` |
| `CHCFG <ch> FILTER BLOCK <oct>` | 黑名单屏蔽指定 Label | `CHCFG 1 FILTER BLOCK 377` | `OK: Filter rule Blocked on Channel 1` |
| `CHCFG <ch> FILTER CLEAR` | 清空过滤规则 | `CHCFG 1 FILTER CLEAR` | `OK: Filter rule Cleared on Channel 1` |
| `TX <ch> <oct> <sdi> <data> <ssm>` | 自动计算奇校验并发送 | `TX 0 310 1 0x1234 3` | `OK: Sent L:310 SDI:1 DATA:0x1234 RAW:0xE048D013` |
| `TXRAW <ch> <hex_word>` | 直接发射 32 位原始十六进制字 | `TXRAW 0 0x812345C8` | `OK: Queued raw word 0x812345C8 for CH0` |

---

## 三、 硬件引脚参考与自检接线

* **默认通道映射**：
  * Channel 0 (TX): `GP0` (TX_A), `GP1` (TX_B)
  * Channel 1 (RX): `GP2` (RX_A), `GP3` (RX_B)
* **免收发芯片自测环回**：
  使用杜邦线短接 `GP0 -> GP2` 与 `GP1 -> GP3`，运行 `python tools/pico429.py bit` 即可完成完整电气闭环自检。
