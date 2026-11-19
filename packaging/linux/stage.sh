#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 5 ]]; then
    echo "Usage: $0 STAGE GUI HELPER XRAY XRAY_LICENSE" >&2
    exit 2
fi
stage=$1
gui=$2
helper=$3
xray=$4
license=$5
packaging=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$packaging/../.." && pwd)

install -Dm755 "$gui" "$stage/usr/bin/tangledlink-desktop"
install -Dm755 "$helper" "$stage/usr/lib/tangledlink-desktop/tangledlink-tun-helper"
install -Dm755 "$xray" "$stage/usr/lib/tangledlink-desktop/xray"
install -Dm644 "$license" "$stage/usr/share/licenses/tangledlink-desktop/Xray-LICENSE"
install -Dm644 "$packaging/tangledlink-desktop.desktop" "$stage/usr/share/applications/tangledlink-desktop.desktop"
install -Dm644 "$packaging/pro.tngl.desktop.policy" "$stage/usr/share/polkit-1/actions/pro.tngl.desktop.policy"
install -Dm644 "$repo/desktop/icons/icon.png" "$stage/usr/share/icons/hicolor/128x128/apps/tangledlink-desktop.png"
