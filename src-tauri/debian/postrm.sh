#!/bin/sh
# deb postrm：卸载后刷新缓存。
#
# 用户数据（~/.local/share/com.lover.lyciaplayer 的曲库数据库与封面缓存、
# ~/.cache/com.lover.lyciaplayer 的 WebKit 缓存）刻意保留——符合 XDG 惯例，
# 重新安装或升级后曲库与设置可完整恢复。如需彻底清除请手动删除上述目录。
set -e

if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database -q /usr/share/applications 2>/dev/null || true
fi

if command -v ldconfig >/dev/null 2>&1; then
    ldconfig 2>/dev/null || true
fi

exit 0
