#!/bin/sh
# deb postinst：安装 / 升级完成后刷新桌面集成缓存。
# 任何一步失败都不应阻断安装，故全部容错处理。
set -e

if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database -q /usr/share/applications 2>/dev/null || true
fi

if command -v update-mime-database >/dev/null 2>&1; then
    update-mime-database /usr/share/mime 2>/dev/null || true
fi

if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    for icon_dir in /usr/share/icons/hicolor /usr/share/icons; do
        if [ -d "$icon_dir" ]; then
            gtk-update-icon-cache -q -t -f "$icon_dir" 2>/dev/null || true
        fi
    done
fi

if command -v ldconfig >/dev/null 2>&1; then
    ldconfig 2>/dev/null || true
fi

exit 0
