# WLDAP 使用指导

本文覆盖三件事，按顺序读即可：

1. **烧写** —— 把固件写进 ESP32-C5-MINI
2. **调试** —— 分两种，别混：调试 C5 自身固件 / 用 C5 去调试目标 MCU
3. **烧写之后怎么用** —— 配网、控制台、连接目标、更新固件

> 名词约定：**WLDAP** = 这块 C5-MINI 板子跑本固件后的角色（无线 CMSIS-DAP 探针）。**目标 MCU** = 你要调试的那颗芯片（当前主要是 STM32）。

---

## 0. 先分清三条链路

```
① 调试 C5 自身固件
   VS Code ──USB──> C5 内置 USB-JTAG ──> C5 内核
   （只在开发固件时需要）

② 用 WLDAP 调试目标 MCU
   IAR / VS Code ──GDB :3333──> OpenOCD(PC) ──CMSIS-DAP over TCP :4441──> C5 ──SWD──> 目标 MCU

③ 给目标 MCU 烧固件
   与 ② 同一条链路，由 OpenOCD 的 program 命令驱动
```

三条链路互不冲突，但 **① 和 ② 不能同时占用同一个 USB 口做不同的事**：① 用的是 C5 的 USB-JTAG，② 只在 WiFi 上跑。

---

## 1. 硬件准备

| 功能 | C5-MINI 引脚 | 说明 |
|---|---|---|
| SWCLK | GPIO6 | 建议串 100Ω |
| SWDIO | GPIO7 | 建议串 100Ω，100k 上拉 |
| nRESET | GPIO5 | 开漏，可选 |
| GND | GND | **必须共地** |
| 5V | VIN | 4.5–6V，可用充电宝 |
| 目标 UART（可选） | TX=GPIO11 / RX=GPIO12 | 不要用 IO14（IO14 是 USB D+） |
| 状态灯 | GPIO23 | 板载蓝灯，低电平点亮 |

注意：

- 模组的 `RST` 是芯片 EN，不是目标的 nRESET。
- 模组 `3V3` 是内部 LDO 输出，**不要反灌**。
- 内置 USB-JTAG 用 GPIO13/14，与上面 SWD 引脚不冲突。
- C5 与目标必须共地，否则 SWD 时好时坏。

---

## 2. 烧写（把固件写进 C5）

### 2.1 环境

- WSL2 + ESP-IDF 6.1（路径 `/home/john/.espressif/v6.1/esp-idf`）
- 项目：`~/Workspace/WLDAP/wldap_gdb_server`
- 固件版本约束：`main/idf_component.yml` 里 `idf: ">=6.0,<7.0"`，不满足会在依赖解析阶段直接失败

### 2.2 让 WSL 看到板子的 USB

Windows 侧（管理员 PowerShell，装一次）：

```powershell
winget install --exact --interactive dorssel.usbipd-win
```

每次使用：

```powershell
usbipd list                          # 找到 303a:1001  USB JTAG/serial debug unit
usbipd bind   --busid 3-2            # 只需一次
usbipd attach --wsl --busid 3-2 --auto-attach
```

`--auto-attach` 很关键：芯片每次复位 USB 会重新枚举，没有它就得反复插拔。

WSL 侧（一次性权限配置）：

```bash
sudo cp ~/.espressif/tools/openocd-esp32/*/openocd-esp32/share/openocd/contrib/60-openocd.rules /etc/udev/rules.d/
sudo udevadm control --reload-rules
sudo udevadm trigger --subsystem-match=usb --action=change
sudo usermod -aG dialout $USER      # 然后 wsl --shutdown 重开
```

验证：

```bash
ls -l /dev/ttyACM0        # 期望 root:plugdev，且 test -w 通过
```

### 2.3 编译

```bash
cd ~/Workspace/WLDAP/wldap_gdb_server
./scripts/build_wldap.sh
```

产物：`build/build_esp32c5_mini/cmsis_dap_tcp_esp32.bin`

也可以手动：

```bash
python3 "$IDF_PATH/tools/idf.py" --preset board_esp32c5_mini build
```

> 必须带 `--preset board_esp32c5_mini`。不带会 fallback 到第一个 preset（S3），报 `IDF_TARGET esp32s3 vs esp32c5` 不匹配。
>
> 必须用 `python3 .../idf.py` 而不是 `idf.py`：CMake 缓存里记的解释器是 `python3`，用 `python` 会以 `Run 'idf.py fullclean' to start again` 退出。

### 2.4 烧写

```bash
./scripts/flash_wldap.sh                     # 烧完自动进 monitor
PORT=/dev/ttyUSB0 ./scripts/flash_wldap.sh   # CH340/CP210x 时
```

手动等价命令：

```bash
idf.py --preset board_esp32c5_mini -p /dev/ttyACM0 flash
```

烧写的三块内容（`flash_args`）：

| 地址 | 文件 | 何时需要 |
|---|---|---|
| `0x2000` | `bootloader.bin` | 随 `flash` 自动写入 |
| `0x8000` | `partition-table.bin` | 随 `flash` 自动写入 |
| `0x10000` | `cmsis_dap_tcp_esp32.bin` | 日常改代码用 `app-flash` 可只写这块 |

**不需要按 BOOT**，也**不需要插拔**：

- `esptool` 通过 USB-Serial/JTAG 自己发复位进下载模式（`--before default-reset`），烧完 `--after hard-reset` 自动重启跑 app。
- BOOT 键只在需要强制 ROM 下载模式（芯片跑飞、USB 不稳）时用：按住 BOOT → 按 RST → 松开。
- 若确实按过 BOOT 上电，芯片停在下载模式不跑 app，松开 BOOT 按一下 RST 即可。

monitor 退出：`Ctrl+]`（**不是** `Ctrl+C`）。

---

## 3. 调试

### 3.1 调试 C5 自身固件（开发 WLDAP 固件时用）

前置：2.2 的 USB 直通已完成。

`.vscode/launch.json` 已配置好，F5 即可：

```json
"idf.openOcdConfigs": ["board/esp32c5-builtin.cfg"]
```

行为：起 OpenOCD（`esp_usb_jtag`）→ 连 `:3333` → 停在 `app_main`。

也可以用命令行先验证链路：

```bash
openocd -f board/esp32c5-builtin.cfg
```

看到下面这些就算通：

```
Info : esp_usb_jtag: Device found. Base speed 24000KHz
Info : JTAG tap: esp32c5.tap0 tap/device found: 0x00017c25
Info : [esp32c5] Chip revision v1.2
Info : [esp32c5] starting gdb server on 3333
```

> `Warn : Failed to read memory via program buffer` 是 C5 的正常提示（`progbufsize=2` 太小，自动回退），只要后面有 `Examination succeed` 就忽略。

注意事项：

- 调试会话进行中**不要**同时开 `idf.py monitor`，两者共用同一个 USB-Serial-JTAG。
- **烧写前先 `Shift+F5` 停止调试**，否则 OpenOCD 占着 3333 和 libusb 句柄，扩展会走 JTAG flash 分支并卡住（`Can't perform JTAG flash...`）。
- VS Code 的烧写方式设为 **UART**（`idf.flashType` = `UART`），让它走 esptool + `/dev/ttyACM0`，彻底绕开 OpenOCD。

### 3.2 用 WLDAP 调试 / 烧写目标 MCU（日常用途）

#### 3.2.1 流程

```
1. C5 上电 → 连 WiFi → 拿到 IP
2. 接线：SWCLK/SWDIO/nRESET/GND，目标自己供电
3. PC 起 OpenOCD：连 <IP>:4441，对外开 :3333
4. IAR / VS Code 连 localhost:3333
```

#### 3.2.2 主机侧 OpenOCD

```bash
cd ~/Workspace/WLDAP/wldap_gdb_server
openocd -c "set WLDAP_IP 192.168.1.42" \
        -f host/openocd-wldap.cfg \
        -f target/stm32f1x.cfg
```

成功标志：

```
Info : CMSIS-DAP: Connecting to 192.168.1.42:4441 using TCP backend
Info : CMSIS-DAP: using minimum timeout of 300 ms for TCP packets
Info : Listening on port 3333 for gdb connections
```

给目标直接烧固件：

```bash
openocd -c "set WLDAP_IP 192.168.1.42" \
        -f host/openocd-wldap.cfg \
        -f target/stm32f1x.cfg \
        -c "program firmware.elf verify reset exit"
```

调 SWCLK：cfg 里 `adapter speed 1000`，稳定后逐步上调；**上限约 6 MHz**（固件是软件位操作，不是 SPI 40 MHz）。

#### 3.2.3 IAR 9

- Debugger 选 **GDB Server**
- Host `127.0.0.1`，Port `3333`
- 目标器件按实际 MCU 选
- 先启动 OpenOCD，再在 IAR 里 Download & Debug

#### 3.2.4 VS Code（Cortex-Debug）

```json
{
  "type": "cortex-debug",
  "request": "attach",
  "servertype": "external",
  "gdbTarget": "localhost:3333",
  "name": "WLDAP attach"
}
```

---

## 4. 烧写之后如何使用设备

### 4.1 看指示灯判断状态

| 灯效 | 含义 | 该做什么 |
|---|---|---|
| 上电连闪 4 次 | 启动 | 等待 |
| 1 Hz 慢闪 | 正在连 WiFi | 检查 SSID/密码；超过 20 次重试会自动重启 |
| 快闪 3 下 + 长停 | WiFi 失败 | 用串口控制台重新 `wifi` 配置 |
| 每 2 秒一次短闪 | **就绪**，WiFi 通了，无客户端 | 可以连 OpenOCD |
| 常亮 | 已连接调试器 | 正常工作中 |
| 快速闪烁 | SWD 有数据交互 | 单步/读写中 |
| 近似常亮 + 每 0.5 s 一个暗点 | 批量传输（**烧写中**） | 不要断电 |
| 快闪 6 下 + 停顿 | 致命错误，即将重启 | 看串口日志 |

### 4.2 串口控制台（USB，最可靠）

```bash
python3 -m serial.tools.miniterm /dev/ttyACM0 115200
# 或
screen /dev/ttyACM0 115200
# 或
idf.py --preset board_esp32c5_mini -p /dev/ttyACM0 monitor
```

`miniterm` 退出 `Ctrl+]`，`screen` 退出 `Ctrl+A` 再 `K`。

### 4.3 网络控制台（WiFi，方便但依赖网络）

```bash
nc <C5_IP> 4440
# 或 telnet <C5_IP> 4440
```

端口来自 `CONFIG_ESP_DAP_SOCKET_CONSOLE_TCP_PORT=4440`，带 keepalive（10 s）。

### 4.4 命令参考

| 命令 | 说明 |
|---|---|
| `help` | 列出可用命令 |
| `status` | 报告 WiFi 状态、IP、socket 控制台端口、DAP 端口 |
| `wifi "<ssid>" "<password>" [auth_mode]` | 设置 WiFi 并持久化到 NVS；`auth_mode` 取 `open/wep/wpa/wpa2/wpa3` |
| `reboot` | 重启（改了 `wifi` 后必须重启生效） |
| `uart ...` | UART 桥配置（当前板级默认关闭） |
| `adc ...` | ADC 流（当前板级默认关闭） |

典型首次配置：

```
wifi "MyHome2G" "my-password" wpa2
reboot
```

`status` 输出示例：

```
Connected to WiFi SSID: 'MyHome2G'. RSSI: -52 dBm
IP address: 192.168.1.42
Socket console: listening on port 4440.
cmsis_dap_tcp 1: JTAG/SWD, port 4441. GPIO (weakest): SWCLK=6 SWDIO=7 NRESET=5
```

> 默认编译进去的是 `myssid` / `mypassword`，必须改，否则会一直重试到重启。

### 4.5 连接目标并下载

1. 按 3.2.2 起 OpenOCD（确认 `Listening on port 3333`）
2. IAR / VS Code 连 `127.0.0.1:3333`
3. Download → 观察状态灯进入"批量传输"档
4. 需要复位时用 nRESET（GPIO5），或 OpenOCD 的 `reset` 命令

### 4.6 更新 C5 自身的固件

改完代码后重复第 2 步即可。因为配了 `--auto-attach`，**不需要拔插 USB**：

```bash
./scripts/flash_wldap.sh
```

WiFi 凭据存在 NVS，重新烧写 `app` 不会丢；只有 `erase-flash` 才会清掉。

---

## 5. 故障排查

| 现象 | 原因 | 处理 |
|---|---|---|
| `LIBUSB_ERROR_ACCESS` / `could not find or open device` | udev 权限不足 | 装 `60-openocd.rules`，确认用户在 `plugdev` |
| `Unsupported DTM version: -1` | 上一条的连带结果，不是芯片问题 | 先修权限 |
| `No DFU capable USB device available` | esptool 没看到 `/dev/ttyACMx` | 检查 `usbipd attach`、`ls /dev/ttyACM0` |
| `Permission denied` 打开 `/dev/ttyACM0` | 不在 `dialout` 组 | `usermod -aG dialout` 后重开 WSL |
| 每次复位后 ttyACM0 消失 | usbipd 未 auto-attach | `usbipd attach --wsl --busid X --auto-attach` |
| `Can't perform JTAG flash, because OpenOCD server is not running!` | 扩展走了 JTAG flash，且 OpenOCD 未起 | `Shift+F5` 停调试；`idf.flashType` 改 `UART` |
| OpenOCD 反复刷 `could not find or open device` | 调试会话没停，设备已重新枚举 | `pkill -f openocd` 后重连 |
| 下次烧写必须重启 VS Code | 上一会话的 OpenOCD 未释放 | 同上；养成"先停调试再烧写" |
| `command mismatch` | WiFi RTT 抖动 / 超时太短 | `min_timeout` 改 500；靠近 AP；优先 5 GHz |
| 烧写成功但灯不闪 | 可能仍停在 BOOT 下载模式 | 松开 BOOT 按 RST |
| WiFi 连上后频繁重启 | 服务启动后掉线会主动 reboot（设计如此） | 改善信号/功耗设置 |

---

## 6. 附录

### 6.1 端口

| 端口 | 归属 | 用途 |
|---|---|---|
| 4441 | C5 | CMSIS-DAP over TCP |
| 4440 | C5 | socket 控制台 |
| 3333 | PC OpenOCD | GDB Remote |
| 4444 | PC OpenOCD | telnet 监控 |
| 6666 | PC OpenOCD | TCL |

### 6.2 关键 Kconfig（`sdkconfig.board_esp32c5_mini`）

```
CONFIG_ESP_DAP_1_SWD_SUPPORTED=y
CONFIG_ESP_DAP_1_JTAG_SUPPORTED=n
CONFIG_ESP_DAP_1_GPIO_SWCLK_TCK=6
CONFIG_ESP_DAP_1_GPIO_SWDIO_TMS=7
CONFIG_ESP_DAP_1_GPIO_NRESET=5
CONFIG_ESP_CONSOLE_USB_SERIAL_JTAG=y
CONFIG_WLDAP_STATUS_LED=y
CONFIG_WLDAP_STATUS_LED_GPIO=23
CONFIG_WLDAP_STATUS_LED_ACTIVE_LOW=y
CONFIG_ESP_DAP_PRODUCT_ID="CMSIS-DAP-TCP ESP32-C5-MINI WLDAP"
```

### 6.3 已知限制

- 只支持 SWD，不支持 JTAG（第二阶段再说）。
- 不支持 SWO；要 printf 用 UART 桥或目标自己的串口。
- 同时只允许一个 OpenOCD 客户端。
- 吞吐受 WiFi 影响，C5 尚未实测。
- Keil 传统上只认 USB 探针，需 pyOCD GDB Server 或 USB/IP，属后续工作。
