# ARINC 429 Pico 2 W 软件使用与上位机通信手册

本手册详细说明如何编译与烧录 Pico 2 W 固件，通过纯 Rust 高吞吐上位机与 MCP 智能体服务进行 12 通道全量配置与数据交互，以及纯二进制 USB CDC 通信协议规范。

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

## 二、 纯 Rust 高吞吐上位机引擎与 MCP 智能体服务 (`crates/pico429`)

Pico-429 上位机现已全面转向原生 Rust 架构，提供高吞吐发送引擎（支持 100 kbps 满载流传输）、双通道双工并发、当前值表（CVT）快速索引、环形历史缓冲以及原生 Model Context Protocol (MCP) 服务。

### 1. 编译与安装

直接通过 Cargo 运行或安装到系统 PATH：
```bash
# 直接运行开发版
cargo run --package pico429 -- --help

# 编译并运行 Release 高性能优化版 (默认启动 Windows 1ms 硬件定时器增强)
cargo run --release --package pico429 -- status

# 安装为全局命令行工具
cargo install --path crates/pico429
```

### 2. 命令行核心子命令速查

```bash
# 1. 连通性与通道状态查询 (自动探测 Pico-429 串口)
cargo run --release --package pico429 -- status
# 亦可显式指定端口：
cargo run --release --package pico429 -- --port COM3 status

# 2. 模式切换 (run 物理运行模式 / config 高阻维护模式)
cargo run --release --package pico429 -- config mode run
cargo run --release --package pico429 -- config mode config

# 3. 单通道物理配置与过滤规则设定
# 将 Channel 0 配置为 TX、高速 100k
cargo run --release --package pico429 -- config channel 0 --dir tx --speed high
# 将 Channel 1 配置为 RX、低速 12.5k
cargo run --release --package pico429 -- config channel 1 --dir rx --speed low

# 4. 固件 Profile 固化与 TOML 加载 (片上 4 槽位 Flash 持久化)
# 将本地 TOML 文件批量下发至硬件并持久化至 Flash 槽位 0
cargo run --release --package pico429 -- config profile apply profiles/loopback_ch0_ch1.toml --slot 0
# 查询片上 4 个 Flash 槽位状态、校验和与自启动标记
cargo run --release --package pico429 -- config profile list
# 从 Flash 槽位 0 恢复配置并立即激活物理总线
cargo run --release --package pico429 -- config profile load 0
# 将 Flash 槽位 0 设定为默认开机自启动 Profile (断电不丢失)
cargo run --release --package pico429 -- config profile boot 0
# 清除开机自启动标记 (恢复出厂静默基态)
cargo run --release --package pico429 -- config profile clear-boot
# 擦除指定 Flash 槽位
cargo run --release --package pico429 -- config profile clear 0

# 5. 单字发送 (自动计算物理奇偶校验位)
cargo run --release --package pico429 -- send --channel 0 --label 310 --data 0x1234 --sdi 1 --ssm 3 --parity odd

# 6. 高吞吐连续流传输 (支持指定 32 位原始字与线速速率 high/low)
cargo run --release --package pico429 -- stream --channel 0 -n 10000 --speed high --data 0xE048D013

# 7. 实时总线遥测监听 (分析仪模式，实时格式化显示 Label、Data、Parity)
cargo run --release --package pico429 -- listen --channel 1 -n 100

# 8. 物理闭环自检 (BIT, 校验 8 组基准航电向量与回环完整性)
cargo run --release --package pico429 -- bit --tx 0 --rx 1 --count 20
```

### 3. 原生 MCP 服务器模式 (Model Context Protocol)

`pico429` 原生内置 MCP 服务器，为 AI 编程助手（如 Claude Desktop、Cursor、VS Code MCP、Antigravity）提供对 ARINC 429 硬件的完整控制与监测能力：

```bash
# 启动标准输入/输出 (stdio) MCP 服务
cargo run --release --package pico429 -- mcp serve
```

#### MCP 客户端配置示例

在 Claude Desktop 或 Cursor 的 `mcpServers` 配置段中添加：
```json
{
  "mcpServers": {
    "pico429": {
      "command": "cargo",
      "args": ["run", "--release", "--manifest-path", "C:/Users/lihan/Work/pico-429/Cargo.toml", "--package", "pico429", "--", "mcp", "serve"]
    }
  }
}
```

#### MCP 工具集 (9 项原子工具)
1. `pico429_get_status`：查询固件连接、运行模式与 12 通道物理状态。
2. `pico429_configure_channels`：批量原子配置通道方向、速率与运行模式。
3. `pico429_send_word`：发送单帧 ARINC 429 航电字。
4. `pico429_stream_send`：在后台异步执行指定速率与数量的流式发送任务。
5. `pico429_stream_status`：查询后台流式发送任务的实时进展（已发字数、状态）。
6. `pico429_abort_stream`：紧急终止发送任务，排空固件 FIFO 并回收额度。
7. `pico429_fetch_telemetry`：从 32,768 深度历史环形缓冲区分页提取接收数据。
8. `pico429_get_latest_labels`：从当前值表 (CVT) 中无锁瞬时读取最新航电物理值。
9. `pico429_run_bit`：执行硬件通道闭环自检并输出统计分析。

### 4. 运行状态与指示灯说明
> ⚠️ **当前状态说明**：
> 当前阶段固件专注于 USB 12 通道物理全双工硬实时收发与确定性时序。因 Pico 2 W 板载 LED 挂接于外部 CYW43439 协处理器芯片，直接驱动需要消耗 1 个 PIO 状态机，与 12 通道满载约束冲突，因此当前固件已完全精简移除 LED 闪烁任务。
> 运行模式与通道健康状态统一通过上位机 CLI 极速查询：`cargo run --release --package pico429 -- status` 或 `pico429 status`。
> 物理 LED 指示功能已列入后续技术方案规划（详见 `plan/todo/PICO2_MIGRATION_AND_STATUS_LED.md`），待后续迁移至标准版 Pico 2（直接原生直驱片内 GPIO 25）或 RP2350B 硬件时重新使能。

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

### 1. 16 字节通用对称二进制帧结构定义 (BinaryFrame)

| 偏移 (Bytes) | 长度 | 字段名称 | 类型 | 说明 |
| :---: | :---: | :--- | :--- | :--- |
| **0..1** | 2 | `magic` | `[u8; 2]` | 固定协议魔数 **`[0x41, 0x29]`** (ASCII `"A)"`，小端整型 `0x2941`) |
| **2** | 1 | `type_and_ch`| `u8` | 高 4 位为 `frame_type`，低 4 位为 `channel` (0~15)<br>• `0x01`: ARINC_DATA (实时业务遥测)<br>• `0x02`: CMD_ACK (控制命令确认/应答)<br>• `0x03`: ASYNC_EVENT (异步告警事件)<br>• `0x04`: CMD_REQ (上位机控制请求)<br>• `0x05`: STREAM_TX_DATA (高吞吐连续发送流，免逐帧 ACK)<br>• `0x06`: STREAM_CREDIT (固件信用额度回传，带空闲槽位与累计消费计数) |
| **3** | 1 | `flags` | `u8` | 语义依帧类型复用：<br>• `CMD_REQ`: 控制操作码 (`Opcode`)<br>• `CMD_ACK`: 应答状态码 (`AckStatus`)<br>• `ARINC_DATA`: 硬件标志 (`bit0`=奇偶错, `bit1`=100k高速, `bit2`=间隙错, `bit3`=发送回显, `bit4`=总线故障) |
| **4..5** | 2 | `seq_num` | `u16` (小端) | 16 位请求序号（ACK 帧原样回传，用于匹配与 RTT 统计） |
| **6..9** | 4 | `payload_0` | `u32` (小端) | 载荷 0：针对 ARINC_DATA 为微秒级硬件时间戳 (`timestamp_us`)；针对控制帧为参数 0 |
| **10..13**| 4 | `payload_1` | `u32` (小端) | 载荷 1：针对 ARINC_DATA 为 32 位原始 ARINC 429 字 (`raw_word`)；针对控制帧为参数 1 |
| **14..15**| 2 | `crc16` | `u16` (小端) | 针对字节 2..13 的 CRC16-CCITT 校验码（多项式 `0x1021`，初值 `0xFFFF`） |

### 2. 操作码 (Opcode) 与应答状态码 (AckStatus)

#### 上位机请求操作码 (Opcode，填入 CMD_REQ 的 flags 字段)：
* `0x01` (`PING`)：连通性心跳探针。
* `0x02` (`SET_CHANNEL_CFG`)：配置单通道启停、方向与速率 (payload_0 包含 4-bit nibble)。
* `0x03` (`SET_FILTER`)：配置指定通道硬件位图过滤 (`0`=清空, `1`=放行Label, `2`=屏蔽Label)。
* `0x04` (`TX_RAW_WORD`)：向下位机发送 32 位 ARINC 字 (payload_1 为原始数据字)。
* `0x05` (`SET_OPERATING_MODE`)：配置运行模式 (`0`=Config 维护模式, `1`=Operational 运行模式)。
* `0x07` (`CFGBATCH`)：12 通道原子批量重构 (payload_0 包含 CH0~7，payload_1 包含 CH8~11)。
* `0x08` (`QUERY_STATUS`)：查询运行模式与 12 通道状态 (ACK 的 channel 承载模式，payload_0/1 承载通道状态掩码)。
* `0x09` (`PROFILE_OP`)：片上 Flash 配置方案操作 (`0`=List, `1`=Save, `2`=Load, `3`=Boot, `4`=ClearBoot)。
* `0x0B` (`ABORT_TX`)：紧急终止指定通道发送流，固件原子排空发送队列与 PIO FIFO，并在 ACK 帧载荷中返回丢弃字统计 (`payload_0`)。

#### 下位机应答状态码 (AckStatus，填入 CMD_ACK 的 flags 字段)：
* `0x00` (`OK`)：操作成功完成。
* `0x01` (`ERR_UNKNOWN_CMD`)：未知或不支持的操作码。
* `0x02` (`ERR_INVALID_PARAM`)：参数错误（如通道号超限、非法模式等）。
* `0x03` (`ERR_MODE_CONFLICT`)：模式冲突（例如在 Config 模式下尝试发字）。
* `0x04` (`ERR_FLASH`)：片上 Flash 读写擦除异常、校验失败或槽位为空。
* `0x05` (`ERR_BUFFER_FULL`)：硬件缓冲区或发送队列已满。

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
  cargo run --release --package pico429 -- bit --tx 0 --rx 1
  # 或直接运行全局命令：
  pico429 bit --tx 0 --rx 1
  ```
  即可完成微秒级纯二进制闭环自检。
