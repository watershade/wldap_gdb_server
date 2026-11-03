#!/usr/bin/env bash
set -euo pipefail

# Flash + monitor WLDAP on ESP32-C5-MINI.
#
# Same activate-script quirk as build_wldap.sh: the ESP-IDF activate script must
# be sourced from a shell whose $0 is a shell name, with nounset off.
IDF_ACTIVATE="${IDF_ACTIVATE:-/home/john/.espressif/tools/activate_idf_v6.1.sh}"
PROJECT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
BUILD_DIR="${BUILD_DIR:-$PROJECT_DIR/build/build_esp32c5_mini}"
PORT="${PORT:-/dev/ttyACM0}"

[ -f "$IDF_ACTIVATE" ] || { echo "IDF activate script not found: $IDF_ACTIVATE" >&2; exit 1; }

export IDF_ACTIVATE PROJECT_DIR BUILD_DIR PORT

bash -c '
set +u
source "$IDF_ACTIVATE"
set -euo pipefail
cd "$PROJECT_DIR"
python3 "$IDF_PATH/tools/idf.py" --preset board_esp32c5_mini \
  -B "$BUILD_DIR" -p "$PORT" flash monitor
'
