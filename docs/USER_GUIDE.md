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

# 9. 12 通道 TOML 配置文件管理与一键加载 (模式 B)
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

# 10. 发送单个构造字 (Label 310, 数据 0x1234)
uv run --with pyserial python tools/pico429.py send --label 310 --data 0x1234

# 11. 彩色流式监听并解码接收数据
uv run --with pyserial python tools/pico429.py listen
```

---

### 工具 B：通用串口助手 (PuTTY / SSCOM / XCOM 等)
串口波特率配置为 115200（固件默认 ASCII 交互）：

| 指令格式 | 功能说明 | 示例 | 预期响应 |
| :--- | :--- | :--- | :--- |
| `PING` | 极速连通性心跳 | `PING` | `PONG` |
| `STATUS [ch]` | 查询固件状态与当前通道方向 | `STATUS` | `STATUS: Pico-429 12CH Online...` |
| `MODE <BIN\|ASCII>` | 切换数据上报格式 | `MODE BIN` | `OK: Switched to BINARY mode` |
| `PAUSE` | 安全挂起全部 12 通道物理收发 | `PAUSE` | `OK: All channels paused` |
| `RESUME` | 恢复全部 12 通道物理收发 | `RESUME` | `OK: All channels resumed` |
| `CFGDUMP` | 导出全部 12 通道紧凑方向与速率 | `CFGDUMP` | `CFGDUMP: 0:TX:HIGH 1:RX:HIGH ...` |
| `CFGBATCH <spec>` | 批量原子应用通道重构 | `CFGBATCH 0=TX:HIGH,1=RX:HIGH` | `OK: Batch configured 2 channels` |
| `PROFILE LIST` | 列出片上 Flash 4 个槽位与自启动 | `PROFILE LIST` | `PROFILES: Boot Slot: Some(0)...` |
| `PROFILE SAVE <slot> [name]` | 保存当前配置至指定 Flash 槽位 | `PROFILE SAVE 0 Bench` | `OK: Profile saved to slot 0` |
| `PROFILE LOAD <slot>` | 从 Flash 槽位加载并激活配置 | `PROFILE LOAD 0` | `OK: Profile loaded from slot 0` |
| `PROFILE BOOT <slot>` | 设置上电默认自启动 Flash 槽位 | `PROFILE BOOT 0` | `OK: Default boot profile set to slot 0` |
| `CHCFG <ch> SPEED <HIGH\|LOW>` | 切换指定通道波特率 | `CHCFG 0 SPEED HIGH` | `OK: Channel 0 set to HIGH` |
| `CHCFG <ch> FILTER ALLOW <oct>` | 白名单放行指定 Label | `CHCFG 1 FILTER ALLOW 310` | `OK: Filter rule Allowed on Channel 1` |
| `CHCFG <ch> FILTER BLOCK <oct>` | 黑名单屏蔽指定 Label | `CHCFG 1 FILTER BLOCK 377` | `OK: Filter rule Blocked on Channel 1` |
| `CHCFG <ch> FILTER CLEAR` | 清空过滤规则 | `CHCFG 1 FILTER CLEAR` | `OK: Filter rule Cleared on Channel 1` |
| `TX <ch> <oct> <sdi> <data> <ssm>` | 自动计算奇校验并发送 | `TX 0 310 1 0x1234 3` | `OK: Sent L:310 SDI:1 DATA:0x1234 RAW:0xE048D013` |
| `TXRAW <ch> <hex_word>` | 直接发射 32 位原始十六进制字 | `TXRAW 0 0x812345C8` | `OK: Queued raw word 0x812345C8 for CH0` |

---

## 三、 硬件引脚分配与环回接线

* **12 通道物理引脚分布**：
  * Channel 0: `GP0` (Pin A), `GP1` (Pin B) [出厂默认: TX]
  * Channel 1: `GP2` (Pin A), `GP3` (Pin B) [出厂默认: RX]
  * Channel 2..11: `GP4..GP23`（相邻奇偶引脚对）[出厂默认: RX 被动监听]
* **免收发芯片自测环回**：
  使用杜邦线短接 `GP0 -> GP2` 与 `GP1 -> GP3`，运行 `python tools/pico429.py bit` 即可完成完整电气闭环自检。
