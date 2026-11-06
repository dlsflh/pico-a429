# ARINC 429 Pico 2 W 软件使用与上位机通信手册

本手册详细说明如何编译与烧录 Pico 2 W 固件，通过 Python 极速工具链进行 12 通道全量配置与数据交互，以及纯二进制 USB CDC 通信协议规范。

---

## 一、 固件编译与烧录

### 1. 编译生成 UF2 镜像
固件采用 Rust `thumbv8m.main-none-eabihf` 交叉编译，并使用 `picotool` 转换为 UF2 固件：
```bash
# 1. 编译 Release 优化固件
cargo build --package pico-429-firmware --target thumbv8m.main-none-eabihf --release

# 2. 转换为 Pico 2 识别的 UF2 格式 (picotool 必须使用 -t elf 显式指定输入文件类型)
picotool uf2 convert target/thumbv8m.main-none-eabihf/release/pico-429-firmware -t elf pico429.uf2
```

### 2. 烧录固件
1. 按住 Pico 2 W 板载 **BOOTSEL** 按钮插入 USB，电脑将挂载为 `RPI-RP2` 大容量存储磁盘；
2. 将生成的 `pico429.uf2` 拖入该磁盘，芯片自动重启并进入默认安全基态。

---

## 二、 上位机极速工具使用指南 (`tools/pico429.py`)

上位机采用 Python 工具链管理，执行时通过 `uv run` 免编译直接调用：

```bash
# 1. 连通性测试 (PING -> PONG，纯二进制 RTT < 0.5ms)
uv run --with pyserial python tools/pico429.py ping

# 2. 查询固件 12 通道物理状态与当前运行模式 (Config / Operational)
uv run --with pyserial python tools/pico429.py status

# 3. 硬件自收自发闭环自检 (BIT) (短接 GP0->GP2 与 GP1->GP3，RTT < 1ms)
uv run --with pyserial python tools/pico429.py bit

# 4. 实时交互测试终端 (开启常驻会话，输入测试字即时回显并统计微秒级 RTT)
uv run --with pyserial python tools/pico429.py interactive

# 5. 动态单通道控制 (启停、方向、速率)
# 提示: 添加 --run 可在配置完成后自动切换为 Operational 运行模式 (总线实时收发)
uv run --with pyserial python tools/pico429.py config 0 --enable --dir TX --speed high --run
uv run --with pyserial python tools/pico429.py config 1 --enable --dir RX --speed high
uv run --with pyserial python tools/pico429.py config 2 --disable

# 6. 单独调整通道速率 (high: 100 kbps, low: 12.5 kbps)
uv run --with pyserial python tools/pico429.py speed 0 low

# 7. 配置 Label 硬件过滤规则 (白名单 / 黑名单 / 清空)
uv run --with pyserial python tools/pico429.py filter 1 --allow 310
uv run --with pyserial python tools/pico429.py filter 1 --block 377
uv run --with pyserial python tools/pico429.py filter 1 --clear

# 8. 运行模式切换 (安全维护静默 / 投入总线运行)
uv run --with pyserial python tools/pico429.py opmode config       # 进入 Config 模式 (总线收发锁定，引脚高阻静默)
uv run --with pyserial python tools/pico429.py opmode operational  # 进入 Operational 模式 (激活物理总线收发)

# 9. 发送 ARINC 429 单字 (自动计算奇偶校验)
# 提示: 添加 --run 可在设备处于 Config 模式时自动激活运行模式后发送
uv run --with pyserial python tools/pico429.py send -c 0 --label 310 --data 0x1234 --run

# 10. 12 通道 TOML 配置文件管理与一键批量下发
# 从硬件当前状态逆向导出 TOML 模板
uv run --with pyserial python tools/pico429.py profile dump -o my_bench.toml

# 按照 TOML 配置文件一键原子重构所有 12 通道 (默认自动激活 Operational 运行模式)
uv run --with pyserial python tools/pico429.py profile apply profiles/avionics_bench.toml
# 若需下发后保持 Config 模式，可添加 --no-run:
# uv run --with pyserial python tools/pico429.py profile apply profiles/avionics_bench.toml --no-run

# 将当前运行配置固化到片上 Flash 槽位 (0..3)
uv run --with pyserial python tools/pico429.py profile save 0 --name "Bench_Test"

# 列出片上 Flash 中的所有槽位信息与当前开机自启槽位
uv run --with pyserial python tools/pico429.py profile list

# 从片上 Flash 槽位加载配置并生效
uv run --with pyserial python tools/pico429.py profile load 0

# 设置指定 Flash 槽位为上电开机自启动方案 (必须确保目标槽位非空且通过校验)
uv run --with pyserial python tools/pico429.py profile boot 0

# 清空上电开机自启动设置 (恢复默认上电静默基态)
uv run --with pyserial python tools/pico429.py profile boot --clear

# 10. 应用预置的标准配置方案
uv run --with pyserial python tools/pico429.py profile apply profiles/bit_loopback.toml    # 闭环自检双通道方案
uv run --with pyserial python tools/pico429.py profile apply profiles/avionics_bench.toml  # 航电多通道综合台架方案
uv run --with pyserial python tools/pico429.py profile apply profiles/default_silent.toml  # 全通道静默恢复方案

# 11. 发送指定航电数据字 (自动计算奇校验)
uv run --with pyserial python tools/pico429.py send --channel 0 --label 310 --data 0x1234 --sdi 1 --ssm 3

# 12. 彩色总线数据监听流
uv run --with pyserial python tools/pico429.py listen
```

---

## 三、 12 通道 TOML 配置文件规范

上位机使用符合声明式语义的 TOML 文件对 12 通道进行批量管理。为了彻底杜绝模式冲突，**策略（过滤模式）与数据（Label 列表）完全解耦**：

```toml
# Pico-429 12-Channel ARINC 429 Configuration Profile
[profile]
name = "Avionics_Flight_Bench"
version = "1.0"
boot_slot = 0          # 可选：指定开机自动加载的 Flash 槽位 (0..3)

[global]
mode = "BIN"           # 纯二进制协议标识

# 通道 0：发送通道 (100 kbps 高速)
[channel.0]
direction = "TX"
speed = "high"

# 通道 1：接收通道，白名单模式 (仅放行指定航电 Label)
[channel.1]
direction = "RX"
speed = "high"
filter_mode = "whitelist"
labels = ["310", "203", "001"]

# 通道 2：接收通道，黑名单模式 (屏蔽特定扰动 Label，其余放行)
[channel.2]
direction = "RX"
speed = "low"
filter_mode = "blacklist"
labels = ["377"]

# 通道 3：接收通道，全放行模式 (无过滤)
[channel.3]
direction = "RX"
speed = "high"
filter_mode = "none"
labels = []

# 通道 4..11：未指定或停用通道
[channel.4]
direction = "DISABLED"
```

### 字段说明与约束：
1. `direction`：`"TX"`（发送）、`"RX"`（接收）、`"DISABLED"`（禁用并弱下拉）。
2. `speed`：`"high"`（100 kbps）、`"low"`（12.5 kbps）。
3. `filter_mode`：
   - `"none"`：全放行（默认）。
   - `"whitelist"`：白名单过滤，仅接收 `labels` 中的报文。
   - `"blacklist"`：黑名单过滤，丢弃 `labels` 中的报文。
4. `labels`：八进制 Label 字符串列表（必须在 `000` ~ `377` 范围内）。
5. ⚠️ **零胶水废弃语法拦截**：旧版 `filter = ["ALLOW ..."]` 语法已彻底弃用，解析器会直接报错阻断（Fail-Fast），禁止在配置文件中出现。

---

## 四、 固件状态机与安全基态

固件采用单核心（Core 0）事件驱动架构，内部定义两种确定性工作状态：

```
       上电复位 (Reset)
             │
      ┌──────┴─────────────────────────┐
      │ 无自启动 Profile                │ Flash 存在 boot_slot
      ▼                                ▼
┌───────────────────────────┐    ┌───────────────────────────┐
│     Config 模式 (0x00)     │    │  Operational 模式 (0x01)  │
│  - 12 通道全部 Disabled   │    │  - 自动加载槽位配置       │
│  - PIO 状态机停用         │    │  - PIO 激活总线收发       │
│  - 引脚高阻弱下拉         │    └───────────────────────────┘
│  - 响应所有配置与重构指令 │                  ▲
└───────────────────────────┘                  │
      │                                        │
      └─── profile apply / opmode operational ─┘
```

1. **默认出厂基态**：
   上电时，若 Flash 中没有设置有效 `boot_slot`，固件默认进入 **Config 模式**：
   - 全部 12 个通道初始化为 `Disabled`，PIO 状态机处于休眠状态；
   - 所有引脚设为高阻浮空弱下拉，防止意外向外部总线灌入脉冲；
   - 允许安全下发 `CFGBATCH`、`SET_CHCFG`、`SET_FILTER`。
2. **Operational 模式**：
   通过上位机下发 `SET_OPMODE (0x01)` 或自动加载自启 Profile 进入此模式，PIO 状态机启动，正常进行航电总线收发与实时监听。

### 硬件级三重联锁 (Triple Lock) 安全保障
为了彻底消除游离于状态机管控之外的野行为，固件在内核与硬件驱动层实施了严格的三重联锁：
1. **指令层硬拦截 (Command Gate)**：在 `Config` 模式下，固件调度层直接拒绝 `TX_RAW_WORD` 发送请求，返回 `ERR_MODE_CONFLICT (0x03)`，从协议入口掐断发送可能。
2. **协程层冻结 (Coroutine Gate)**：在 `ChannelDevice::run()` 异步主循环中，若处于 `Config` 模式，通道协程仅休眠等待控制命令，发送与接收数据管道完全切断，绝不向全局发布或消费任何航电字。
3. **物理引脚与 PIO 硬件锁 (Pin & PIO SM Gate)**：在 `Config` 模式下，全部通道的 PIO 状态机被强制 `set_enable(false)` 停用；TX 引脚被强制切换为 `Direction::In`（高阻浮空弱下拉），物理电气上彻底杜绝任意偶发杂散脉冲外溢。

---

## 五、 纯二进制 USB CDC 通信协议规范 (16 字节定长帧)

为了实现微秒级延迟并消除 MCU 字符串解析带来的内存开销与安全漏洞，固件与上位机之间**全面使用 16 字节定长二进制帧（`BinaryFrame`）** 进行对称通信。

### 1. 16 字节二进制帧结构定义

| 偏移 | 字节数 | 字段名称 | 类型 | 说明 |
| :---: | :---: | :--- | :--- | :--- |
| **0..1** | 2 | `magic` | `u16` (小端) | 固定魔数 **`0x29A4`** (大端网络字节序为 `0xA4, 0x29`) |
| **2** | 1 | `frame_type` | `u8` | 帧类型：`0x01`=ARINC_DATA, `0x02`=CMD_ACK, `0x03`=ASYNC_EVENT, `0x04`=CMD_REQ |
| **3** | 1 | `flags` | `u8` | 状态标志位：`bit0`=奇偶校验错, `bit1`=溢出错, `bit2`=位反转 |
| **4** | 1 | `channel` | `u8` | 通道号 (`0` ~ `11`) |
| **5** | 1 | `opcode_ack` | `u8` | 请求时为 `Opcode`；响应时为 `AckStatus` |
| **6..7** | 2 | `seq_num` | `u16` (小端) | 请求序号（ACK 帧原样回显，用于请求匹配与 RTT 统计） |
| **8..11**| 4 | `payload_0` | `u32` (小端) | 数据载荷 0 / 微秒级硬件时间戳 (`timestamp_us`) |
| **12..15**| 4 | `payload_1` | `u32` (小端) | 数据载荷 1 / 32 位原始 ARINC 429 字 (`raw_word`) |
| **14..15**| 2 | `crc16` | `u16` (小端) | 前 14 字节的 CRC16-CCITT 校验码（多项式 `0x1021`，初值 `0xFFFF`） |

> 📌 **注**：当 `crc16` 占用第 14~15 字节时，`payload_1` 处于第 10~13 字节（总长严格为 16 字节，具体内存布局详见 [`crates/arinc429-core/src/protocol.rs`](file:///crates/arinc429-core/src/protocol.rs)）。

### 2. 操作码 (Opcode) 与应答状态码 (AckStatus)

#### 上位机请求操作码 (Opcode)：
* `0x01` (`PING`)：连通性心跳探针。
* `0x02` (`QUERY_STATUS`)：查询运行模式与 12 通道状态（ACK 中返回 4-bit nibble 打包通道状态）。
* `0x03` (`SET_CHCFG`)：配置单通道启停、方向与速率。
* `0x04` (`SET_SPEED`)：配置单通道波特率 (`0`=100k, `1`=12.5k)。
* `0x05` (`SET_FILTER`)：配置单通道硬件位图过滤 (`0`=清空, `1`=放行Label, `2`=屏蔽Label)。
* `0x06` (`SET_OPMODE`)：配置运行模式 (`0`=Config 模式, `1`=Operational 模式)。
* `0x07` (`TX_WORD`)：向下位机发送 32 位 ARINC 字。
* `0x08` (`PROFILE_OP`)：片上 Flash 配置管理 (`0`=List, `1`=Save, `2`=Load, `3`=Boot)。
* `0x09` (`CFGBATCH`)：12 通道原子批量重构（载荷携带 4-bit nibble 压缩掩码）。

#### 下位机应答状态码 (AckStatus)：
* `0x00` (`OK`)：操作成功完成。
* `0x01` (`ERR_INVALID_PARAM`)：参数错误（如通道号超限、非法波特率等）。
* `0x02` (`ERR_TX_FULL`)：发送硬件队列已满。
* `0x03` (`ERR_FLASH`)：片上 Flash 读写擦除异常或校验失败。
* `0x04` (`ERR_CRC`)：上位机下发报文 CRC16 校验不匹配。
* `0x05` (`ERR_BUSY`)：通道正在执行关键操作，拒绝重配。

### 3. 12 通道 4-bit Nibble 压缩编码
在 `CFGBATCH` 与 `QUERY_STATUS` 中，12 个通道的配置被高效压缩为 4-bit nibble：
```
Bit 0: Enable (1=启用, 0=禁用)
Bit 1: IsRx   (1=RX 接收, 0=TX 发送)
Bit 2: HighSpeed (1=100 kbps, 0=12.5 kbps)
Bit 3: 保留 (0)
```
- `payload_0`：编码 Channel 0 ~ Channel 7（共 32 位）
- `payload_1`：编码 Channel 8 ~ Channel 11（共 16 位）

---

## 六、 硬件引脚分配与环回接线

* **12 通道物理引脚分布**：
  * Channel 0: `GP0` (Pin A), `GP1` (Pin B)
  * Channel 1: `GP2` (Pin A), `GP3` (Pin B)
  * Channel 2..10: `GP4..GP21`（相邻奇偶引脚对）
  * Channel 11: `GP26` (Pin 31), `GP27` (Pin 32)
* **免收发芯片自测环回**：
  使用杜邦线短接 `GP0 -> GP2` 与 `GP1 -> GP3`（建议串接 220Ω 电阻），运行：
  ```bash
  uv run --with pyserial python tools/pico429.py bit
  ```
  即可完成微秒级纯二进制闭环自检。
