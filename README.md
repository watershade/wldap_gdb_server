# WLDAP GDB Server

ESP32-C5-MINI 上的无线 CMSIS-DAP 固件：OpenOCD 通过 TCP 连接这块板子，板子再用 SWD 调试目标 MCU。目标是让 IAR / VS Code 在没有 USB 线、没有 USB/IP 驱动的情况下烧写和调试 STM32 一类目标板。

```text
IAR 9 / VS Code
      │ GDB Remote (:3333)
      ▼
OpenOCD（PC）
      │ CMSIS-DAP over TCP (:4441)
      ▼
ESP32-C5-MINI（本仓库固件）
      │ SWD
      ▼
目标 MCU
```

## 来源与继承

本仓库是 **[bkuschak/cmsis_dap_tcp_esp32](https://github.com/bkuschak/cmsis_dap_tcp_esp32)** 的 fork，遵循 Apache License 2.0。

上游本身也不是从零写的：它的 CMSIS-DAP 实现来自 ARM 的 [CMSIS-DAP](https://github.com/ARM-software/CMSIS-DAP) 固件目录，并修改 `DAP_config.h` 以适配 ESP32 的 GPIO 位操作。所以这条代码血统是：

```text
ARM CMSIS-DAP 固件  →  bkuschak/cmsis_dap_tcp_esp32  →  watershade/wldap_gdb_server
```

上游支持 ESP32-S3 / C3 / C6，**不支持 ESP32-C5**。本仓库的主要工作就是补上 C5 这一块，以及配套的主机侧配置和文档。

上游的详细说明（menuconfig 选项、控制台命令、UART bridge、ADC 流、性能数据等）仍然有效，见 [上游 README](https://github.com/bkuschak/cmsis_dap_tcp_esp32#readme)。

## 相对上游的改动

| 文件 | 类型 | 说明 |
|---|---|---|
| `sdkconfig.board_esp32c5_mini` | 新增 | ESP32-C5-MINI 板级配置：目标 `esp32c5`、仅 SWD、SWCLK=GPIO6 / SWDIO=GPIO7 / nRESET=GPIO5、关闭 JTAG、关闭 UART bridge、无 LED、产品名 `CMSIS-DAP-TCP ESP32-C5-MINI WLDAP` |
| `CMakePresets.json` | 修改 | 新增 `board_esp32c5_mini` preset，`binaryDir` 为 `build/build_esp32c5_mini` |
| `host/openocd-wldap.cfg` | 新增 | OpenOCD 侧的 `cmsis-dap backend tcp` 配置，指向 C5 的 IP、端口 4441、`min_timeout 300` |
| `host/README.md` | 新增 | 主机侧接线、构建、烧写、IAR 9 / VS Code 配置说明 |
| `scripts/build_wldap.sh` | 新增 | 走 preset 的构建脚本；处理 ESP-IDF activate 脚本的 `set -u` / `$0` 兼容问题 |
| `scripts/flash_wldap.sh` | 新增 | 烧写 + monitor |
| `.vscode/launch.json` | 新增 | 用 ESP-IDF 扩展调试 C5 自身固件（内置 USB-JTAG） |
| `.vscode/settings.json` | 新增 | 指定 buildPath、OpenOCD 板级配置、clangd |
| `.clangd` | 新增 | 去掉不适用于交叉编译的 `-f*` / `-m*` 参数 |
| `README.md` | 修改 | 本文件；上游原文见链接 |

**没有改动 `main/` 下的任何源码。** 协议解析、SWD 位操作、WiFi、UART bridge 全部是上游与 ARM 的原样代码，版权头保留在各自文件里。C5 的适配完全通过 Kconfig / sdkconfig 完成，这也是上游支持新板子的既定方式（preset + sdkconfig）。

## 硬件

| 功能 | C5-MINI 引脚 | 说明 |
|---|---|---|
| SWCLK | GPIO6 | 建议串 100Ω |
| SWDIO | GPIO7 | 建议串 100Ω，100k 上拉 |
| nRESET | GPIO5 | 开漏，可选，串 100Ω |
| GND | GND | 必须接 |
| 5V | VIN | 4.5–6V，模组内部 LDO 转 3.3V |
| 目标 UART（可选） | TX=GPIO11 / RX=GPIO12 | 不要用 IO14，IO14 是 USB D+ |

注意事项：

- 模组的 `RST` 是芯片 EN，不是目标的 nRESET。
- 模组 `3V3` 是 LDO 输出，不要反灌；要 3.3V 供电请先测反向电流。
- 内置 USB-JTAG 用 GPIO13/GPIO14（D-/D+），与上面的 SWD 引脚不冲突。

## 构建

需要 ESP-IDF。本仓库在 **ESP-IDF v6.1** 上验证，上游在 v6.0.2 上验证。

```bash
# 激活 ESP-IDF
. ~/.espressif/tools/activate_idf_v6.1.sh    # 或你自己的 export.sh

# 方式一：preset
idf.py --preset board_esp32c5_mini build

# 方式二：脚本
./scripts/build_wldap.sh
```

产物在 `build/build_esp32c5_mini/`：

```text
cmsis_dap_tcp_esp32.elf
cmsis_dap_tcp_esp32.bin
```

> 构建目录不要放在 Windows 挂载盘（`/mnt/c`、`/mnt/d`、`/mnt/e`）上。9p 文件系统会让并行编译长时间卡在 D 状态；源码和 build 目录都放在 ext4 的 `$HOME` 下。

## WiFi 配置

默认 SSID 是占位符 `myssid`，必须改，否则板子连不上网。

**方式一：menuconfig**

```text
Component config → CMSIS-DAP configuration → WiFi configuration
```

**方式二：运行时（串口控制台）**

```text
esp32> wifi "你的SSID" "你的密码" wpa2
esp32> reboot
```

## 烧写与运行

```bash
# WSL 下先确认 USB 设备已 attach（usbipd-win）
ls /dev/ttyACM0

./scripts/flash_wldap.sh
# 或
idf.py --preset board_esp32c5_mini -p /dev/ttyACM0 flash monitor
```

启动后串口会打印 IP 地址和监听端口，例如：

```text
cmsis_dap_tcp 1: JTAG/SWD, port 4441. GPIO (weakest): SWCLK=6 SWDIO=7 NRESET=5
```

## 主机侧：OpenOCD

PC 上的 OpenOCD 必须带 `--enable-cmsis-dap-tcp` 编译。用 `host/openocd-wldap.cfg`：

```tcl
adapter driver cmsis-dap
cmsis-dap backend tcp
cmsis-dap tcp host 192.168.1.123
cmsis-dap tcp port 4441
cmsis-dap tcp min_timeout 300
transport select swd
adapter speed 1000
```

```bash
openocd -f host/openocd-wldap.cfg -f target/stm32f1x.cfg
```

看到 `Listening on port 3333 for gdb connections` 即可。

## IAR 9 / VS Code

- **IAR 9**：`Project → Options → Debugger → Setup → Driver` 选 `GDB Server`，Host `127.0.0.1`，Port `3333`。先开 OpenOCD，再连 IAR。
- **VS Code**：Cortex-Debug，`servertype: external` + `gdbTarget: localhost:3333`，或直接 `servertype: openocd`。
- 同一个 OpenOCD 同时只能接一个 GDB 客户端。

## 调试 C5 自身固件

用 VS Code Remote（WSL）+ ESP-IDF 扩展，走 C5 的内置 USB-JTAG，不需要额外探针：

```json
"idf.openOcdConfigs": ["board/esp32c5-builtin.cfg"]
```

`.vscode/launch.json` 已配好，F5 即连 `:3333` 并停在 `app_main`。

注意：`CONFIG_ESP_CONSOLE_USB_SERIAL_JTAG=y` 时，串口控制台和内置 JTAG 共用同一个 USB 控制器。调试会话进行中不要同时开 `idf.py monitor`。

## 已知限制

- 不支持 SWO；需要 printf 请用 UART bridge 或目标板自己的串口。
- SWCLK 上限约 6 MHz（位操作 + 时钟域切换），占空比不固定。
- 同时只允许一个 OpenOCD 客户端。
- 吞吐取决于 WiFi 质量；上游实测 S3 约 200 KB/s（SRAM 写）、C3 约 175 KB/s、C6 约 90 KB/s。C5 尚未实测。
- 只支持 CMSIS-DAP over TCP 的主机工具。Keil 需要 USB/IP 或 elaphureLink，属后续工作。

## 许可

Apache License 2.0，见 [LICENSE](LICENSE)。

上游版权归 Brian Kuschak 及 ARM Limited / Espressif Systems 所有，各源文件保留原始 SPDX 头。本仓库的修改说明见 [NOTICE](NOTICE)。
