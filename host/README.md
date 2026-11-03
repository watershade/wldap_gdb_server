# WLDAP_GDB — 纯软件无线 CMSIS-DAP（第一阶段）

目标：IAR 9 / VS Code 通过 WiFi 使用 ESP32-C5-MINI 做无线 SWD 调试，不经过 USB/IP，不需要 Windows 驱动签名。

## 架构

```text
IAR 9 / VS Code
      │ GDB Remote (:3333)
OpenOCD
      │ CMSIS-DAP over TCP (:4441)
ESP32-C5-MINI
      │ SWD
目标 MCU
```

## 引脚

| 功能 | C5-MINI 引脚 | 说明 |
|---|---|---|
| SWCLK | IO6 / GPIO6 | 串 100Ω |
| SWDIO | IO7 / GPIO7 | 串 100Ω，100k 上拉 |
| nRESET | IO5 / GPIO5 | 开漏，100Ω |
| GND | GND | 必须接 |
| 5V / VIN | VIN | 4.5~6V；目标 5V 或充电宝 |
| 3V3 | 3V3 | ME6211 输出，不建议反灌 |

## 构建

WSL 里使用 IDF 6.1，直接用脚本（自动带 preset）：

```bash
cd ~/Workspace/WLDAP/wldap_gdb_server
./scripts/build_wldap.sh
```

烧写：

```bash
PORT=/dev/ttyACM0 ./scripts/flash_wldap.sh
```

> WSL 里先把 C5 的 USB Serial/JTAG 通过 usbipd-win 直通进来；设备一般是 `/dev/ttyACM0`。

> 如果构建很慢，把项目复制到 `$HOME` 下再构建；`/mnt/e` 源码 + `/mnt/d` IDF 会让并行编译卡在 9p I/O。

## WiFi 配置

首次使用前，在 menuconfig 里设置：

```bash
idf.py -B "$HOME/wldap-build-c5" menuconfig
```

路径：

```text
Component config -> CMSIS-DAP configuration -> WiFi configuration
```

设置 `WiFi SSID` 和 `WiFi Password`，重新 build + flash。

也可以在 USB 串口 console 里运行时设置：

```text
esp32> wifi "your-ssid" "your-password" wpa2
esp32> reboot
```

启动后串口会打印 IP，例如：

```text
cmsis_dap_tcp 1: JTAG/SWD, port 4441. GPIO ... SWCLK=6 SWDIO=7 NRESET=5
```

## OpenOCD

```bash
openocd -f host/openocd-wldap.cfg -f target/stm32f1x.cfg
```

把 `host/openocd-wldap.cfg` 里的 IP 改成 C5 的 IP。OpenOCD 会在 3333 端口提供 GDB server。

## IAR 9

1. `Project > Options > Debugger > Setup > Driver` 选 `GDB Server`。
2. Host：`127.0.0.1`（OpenOCD 在本机）或 OpenOCD 所在机器 IP。
3. Port：`3333`。
4. 先启动 OpenOCD，再让 IAR 连接。
5. 如果 IAR 自带 flash loader 报错，改用 OpenOCD 烧写，IAR 只做调试。

## VS Code

Cortex-Debug 示例：

```json
{
  "type": "cortex-debug",
  "request": "launch",
  "servertype": "openocd",
  "configFiles": [
    "host/openocd-wldap.cfg",
    "target/stm32f1x.cfg"
  ],
  "gdbPath": "/opt/homebrew/bin/arm-none-eabi-gdb",
  "device": "STM32F103C8"
}
```

或者 OpenOCD 已在跑时：

```json
{
  "servertype": "external",
  "gdbTarget": "localhost:3333"
}
```

## 预期性能

| 项目 | 预期 |
|---|---:|
| SWCLK | 1~6MHz |
| SRAM 读写 | 100~200KB/s |
| 64KB Flash | 3~6s |
| 512KB Flash | 13~20s |
| 单步/断点 | 2~10ms/次 |

## 已知限制

- 目前是 GPIO 位操作，SWCLK 上限约 6MHz；不是 Gitee 的 40MHz SPI 加速版。
- 不支持 SWO。
- IAR 的 GDB Server 驱动可能不支持 Live Watch/Trace 等高级功能。
- 同一时间只允许一个 GDB 客户端连接 OpenOCD。
- WiFi 抖动会导致 `command mismatch`；先调大 `min_timeout`，再用专用 AP/5GHz。

## 目录

```text
main/                        固件源码（来自 cmsis_dap_tcp_esp32）
sdkconfig.board_esp32c5_mini C5-MINI 板级配置
CMakePresets.json            增加 board_esp32c5_mini
host/openocd-wldap.cfg       OpenOCD TCP 配置
```
