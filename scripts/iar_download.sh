#!/usr/bin/env bash
# IAR 无头下载 —— 等价于 IDE 里的 Download 按钮，用于自动化验证 WLDAP 调试链路。
#
# 原理：IAR 每次启动调试会话时会在 <EWARM>/settings/ 下生成
#   <proj>.<proj>.general.xcl   处理器 DLL、驱动 DLL、.out、--plugin、--device_macro、
#                               --flash_loader（.board）
#   <proj>.<proj>.driver.xcl    --cpu、.ddf、--device、--drv_communication=TCPIP:...
# 这两份文件就是 cspybat 的完整参数，直接喂给 cspybat 即可复现 IDE 的下载流程，
# 不需要开 GUI，也不会被弹窗挡住。
#
# 用法：
#   ./iar_download.sh                 # 只下载（--download_only）
#   ./iar_download.sh --debug         # 下载后进入调试会话（会挂住，需手动结束）
#   ./iar_download.sh --proj <EWARM 目录，Windows 路径> [--out <out 文件名>]
#
# 前置：WLDAP 的 OpenOCD 必须已在监听（面板点「驱动 OpenOCD」，默认 127.0.0.1:3335）。
set -uo pipefail

IAR_BIN="${IAR_BIN:-/mnt/d/iar/ewarm-9.70.4/common/bin/cspybat.exe}"
EWARM_WIN="${IAR_PROJ_EWARM:-E:\\Workspace\\Embeded\\MCU\\Cranberry_MC_S1\\MC_S1_G474_V1.1\\EWARM}"
OUT_NAME="MC_S1_G474_V1.1"
MODE="--download_only"

while [ $# -gt 0 ]; do
  case "$1" in
    --debug) MODE=""; shift ;;
    --proj)  EWARM_WIN="$2"; shift 2 ;;
    --out)   OUT_NAME="$2"; shift 2 ;;
    *) echo "未知参数: $1" >&2; exit 2 ;;
  esac
done

# Windows 路径 -> WSL 路径，用于定位 xcl
EWARM_UNIX=$(echo "$EWARM_WIN" | sed -e 's|\\|/|g' -e 's|^\([A-Za-z]\):|/mnt/\L\1|')
SETTINGS="$EWARM_UNIX/settings"
[ -d "$SETTINGS" ] || { echo "找不到 settings 目录: $SETTINGS" >&2; exit 1; }

GENERAL=$(ls "$SETTINGS"/*.general.xcl 2>/dev/null | head -1)
DRIVER=$(ls "$SETTINGS"/*.driver.xcl 2>/dev/null | head -1)
[ -n "$GENERAL" ] && [ -n "$DRIVER" ] || {
  echo "没有找到 xcl 文件。先在 IAR 里打开工程并启动一次调试会话（会重新生成）。" >&2
  exit 1
}

# xcl 内部是 Windows 路径，cspybat 需要 Windows 形式
win() { echo "$1" | sed -e 's|^/mnt/\([a-z]\)|\U\1:|'; }
GENERAL_WIN=$(win "$GENERAL")
DRIVER_WIN=$(win "$DRIVER")

cd "$EWARM_UNIX" || exit 1
echo "== 使用 xcl =="
echo "  general: $GENERAL_WIN"
echo "  driver : $DRIVER_WIN"
echo "== cspybat $MODE =="

# shellcheck disable=SC2086
"$IAR_BIN" -f "$GENERAL_WIN" $MODE --backend -f "$DRIVER_WIN"
rc=$?
echo "== cspybat 退出码: $rc =="
exit $rc
