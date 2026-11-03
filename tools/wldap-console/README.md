# WLDAP Console

给没有开发经验的人用的 WLDAP 自检工具：双击一个 exe，浏览器里点几下，就能确认探针是否正常、启动 OpenOCD、看到 IAR 有没有连上来。

- 单个 exe，无外部依赖，离线可编译（只用 Rust 标准库）
- 本地面板 `http://127.0.0.1:8737`

## 功能

| 卡片 | 做什么 |
|---|---|
| 1 连接探针 | 对 `<C5 的 IP>:4441` 做一次 TCP 连接，测量 RTT |
| 2 OpenOCD | 选择目标芯片 / SWCLK / GDB 端口，一键启动或停止，实时日志 |
| 3 环境检查 | 自动找 OpenOCD：程序目录 → PATH → Program Files → 用户目录 → WSL |
| 顶部状态 | OpenOCD 是否在跑、探针是否可达、IDE 是否已连接 |

## 为什么有 WSL 模式

这台开发机上 OpenOCD 只装在 WSL 里（`~/.espressif/tools/openocd-esp32/...`），Windows 侧没有。程序直接调用：

```
wsl.exe -e sh -c ": > '<log>'; exec '<openocd>' -f '<cfg>' -f target/<chip>.cfg"
```

因此：

- 不需要在 Windows 上再装一份 OpenOCD
- 路径由程序自动转换（`D:\x` → `/mnt/d/x`）
- 日志写到 Windows 盘上，界面直接读文件，不跨 WSL 管道

装了 Windows 原生 OpenOCD 的话，把运行方式换成「本机 openocd.exe」即可。

## GDB 端口为什么要自己转发

WSL2 的 localhost 转发（`wslrelay.exe`）会主动探测后端端口来判断能否转发，而 OpenOCD 同时只接受一个 GDB 客户端 —— 探测会把这个名额占掉，真正的 IDE 就连不上，日志里表现为：

```
Info : accepting 'gdb' connection on tcp/3333
Error: attempted 'gdb' connection rejected
```

所以程序在 Windows 上自己监听 `127.0.0.1:<gdb_port>`，再转发到 WSL 的 `<wsl_ip>:<gdb_port>`。副作用是「IDE 已连接」的计数变准了。

同时给 OpenOCD 加 `bindto 0.0.0.0`（WSL 模式下），否则 `wslrelay` 连不上只绑 127.0.0.1 的监听。

## 已知坑：端口残留

旧的 `wslrelay` 进程会一直占着 `127.0.0.1:3333`，即使后端已经没了。这时：

- 程序启动 OpenOCD 会提示「端口被占用」
- 解决：PowerShell 里 `wsl --shutdown`，或把 GDB 端口改成 3334

## 构建

```bash
cargo build --release
```

Windows 上没有 crates.io 也能编（零依赖）。产物在 `target/release/wldap-console.exe`。

> `cargo build` 的工作目录必须是 Windows 盘符路径（`D:\...`），不能是 `\\wsl.localhost\...`。

## 文件

程序在 exe 同目录读写：

| 文件 | 内容 |
|---|---|
| `config.ini` | 上次的设置 |
| `wldap-openocd.cfg` | 每次启动时生成的 OpenOCD 配置 |
| `openocd.log` | OpenOCD 输出（WSL 模式） |

## 交互细节

- 关闭程序窗口即结束 OpenOCD（会 `pkill -f wldap-openocd.cfg` 清残留）
- 启动前会检查 GDB 端口是否已被占用，被占用时在日志里给出提示
