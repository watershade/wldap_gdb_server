#!/usr/bin/env bash
set -euo pipefail

# Build WLDAP for ESP32-C5-MINI.
# Override IDF_ACTIVATE if your ESP-IDF lives elsewhere.
#
# The ESP-IDF activate script is POSIX sh and (a) refuses to load when $0 is not
# a shell name and (b) references $ZSH_VERSION, which trips `set -u`. So run it
# inside `bash -c` (where $0 == "bash") with nounset temporarily off.
IDF_ACTIVATE="${IDF_ACTIVATE:-/home/john/.espressif/tools/activate_idf_v6.1.sh}"
PROJECT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
BUILD_DIR="${BUILD_DIR:-$PROJECT_DIR/build/build_esp32c5_mini}"

[ -f "$IDF_ACTIVATE" ] || { echo "IDF activate script not found: $IDF_ACTIVATE" >&2; exit 1; }

export IDF_ACTIVATE PROJECT_DIR BUILD_DIR

bash -c '
set +u
source "$IDF_ACTIVATE"
set -euo pipefail
cd "$PROJECT_DIR"
idf.py -B "$BUILD_DIR" \
  -DIDF_TARGET=esp32c5 \
  -DSDKCONFIG="$BUILD_DIR/sdkconfig" \
  -DSDKCONFIG_DEFAULTS="sdkconfig.defaults;sdkconfig.board_esp32c5_mini" \
  build
'
