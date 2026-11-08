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
# 1. 连通性测试 (PING -> PONG，纯二进制 RTT < 0.6ms)
uv run --with pyserial python tools/pico429.py ping

# 2. 查询固件 12 通道物理状态与当前运行模式 (Config / Operational)
uv run --with pyserial python tools/pico429.py status

# 3. 运行模式管理 (安全维护静默 / 投入总线运行)
uv run --with pyserial python tools/pico429.py config mode config       # 进入 Config 模式 (总线收发锁定，引脚高阻静默)
uv run --with pyserial python tools/pico429.py config mode run          # 进入 Operational 模式 (激活物理总线实时收发)

# 4. 单通道参数配置 (方向、速率、过滤规则，支持单项与联合指定)
# 缺省规则: --speed 未指定默认为 high; --filter 未指定默认为 none; 白名单无指定 labels 默认全屏蔽，黑名单无指定 labels 默认全放行
uv run --with pyserial python tools/pico429.py config channel 0 --dir tx --speed high --run
uv run --with pyserial python tools/pico429.py config channel 1 --dir rx --filter whitelist --labels 310,203
uv run --with pyserial python tools/pico429.py config channel 2 --disable

# 5. 12 通道 TOML 配置文件与片上 Flash 管理 (支持顶层 flash 或 config flash)
# 应用 TOML 方案并同时持久化到 Flash Slot 0 (包含 32 字节方案名称):
uv run --with pyserial python tools/pico429.py flash apply profiles/loopback_ch0_ch1.toml --slot 0

# 仅应用到当前运行环境而不写入 Flash:
uv run --with pyserial python tools/pico429.py flash apply profiles/loopback_ch0_ch1.toml

# 列出片上 Flash 中的所有槽位方案名称、状态与当前开机自启槽位
uv run --with pyserial python tools/pico429.py flash list

# 从片上 Flash 槽位加载配置并切换为 Operational 运行模式
uv run --with pyserial python tools/pico429.py flash load 0

# 设置指定 Flash 槽位为上电开机自启动方案
uv run --with pyserial python tools/pico429.py flash boot 0

# 清空上电开机自启动设置 (恢复默认上电 CONFIG 静默基态)
uv run --with pyserial python tools/pico429.py flash boot --clear

# 读取 Flash 槽位配置并逆向导出为 TOML 配置文件 (支持单个槽位或 --all 全量导出)
uv run --with pyserial python tools/pico429.py flash dump 0 -o profiles/slot0.toml
uv run --with pyserial python tools/pico429.py flash dump --all --out profiles/

# 清空 Flash 槽位配置 (支持单个槽位或 --all 全部清空)
uv run --with pyserial python tools/pico429.py flash clear 0
uv run --with pyserial python tools/pico429.py flash clear --all

# 6. 硬件内置闭环自检 (BIT)
# 提示: 严格校验当前硬件状态，Config 模式下坚决拒绝；支持 --profile 显式预装载测试方案
uv run --with pyserial python tools/pico429.py bit --profile profiles/loopback_ch0_ch1.toml
uv run --with pyserial python tools/pico429.py bit --tx 0 --rx 1

# 7. 发送指定航电数据字 (自动计算奇校验)
uv run --with pyserial python tools/pico429.py send --channel 0 --label 310 --data 0x1234 --sdi 1 --ssm 3

# 8. 专职总线遥测流监听终端 (ARINC 429 Bus Monitor)
# 监听物理总线接收帧，零线程竞态，微秒级固件捕获时刻高亮呈现 (支持 --count 与 --timeout，按 Ctrl+C 安全退出)
uv run --with pyserial python tools/pico429.py listen
uv run --with pyserial python tools/pico429.py listen --channel 1
uv run --with pyserial python tools/pico429.py listen --channel 1 --count 10 --timeout 5.0
```

### 板载 LED 状态指示说明 (Pico 2 GPIO 25)
* **慢速闪烁 (1 Hz, 500ms 亮 / 500ms 灭)**：`CONFIG` 模式（安全维护静默基态，物理总线锁定，引脚高阻）。
* **快速闪烁 (5 Hz, 100ms 亮 / 100ms 灭)**：`OPERATIONAL` 模式（正常运行模式，物理总线实时收发）。
* **持续常亮 (Solid ON)**：`HARDWARE_FAULT`（硬件底层严重故障，如 Flash 物理写入损坏或总线严重异常）。

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
| **2** | 1 | `type_and_ch`| `u8` | 高 4 位为 `frame_type`，低 4 位为 `channel` (0~15)<br>• `0x01`: ARINC_DATA (实时业务遥测)<br>• `0x02`: CMD_ACK (控制命令确认/应答)<br>• `0x03`: ASYNC_EVENT (异步告警事件)<br>• `0x04`: CMD_REQ (上位机控制请求) |
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
  uv run --with pyserial python tools/pico429.py bit
  ```
  即可完成微秒级纯二进制闭环自检。
