#!/bin/sh
# deb prerm：移除/清除前终止运行中的实例。
#
# $1 取值：remove（卸载）/ purge（清除）/ upgrade（升级）/ deconfigure。
# - 卸载时必须终止实例：否则托盘图标残留、数据目录中可能残留运行态文件。
# - 升级时不干预：Linux 允许替换正在运行的二进制（进程继续使用旧 inode），
#   打断用户的播放体验毫无必要，重启应用后即进入新版本。
set -e

if [ "$1" = "remove" ] || [ "$1" = "purge" ]; then
    pkill -TERM -x lycia_music 2>/dev/null || true
    pkill -TERM -f '/usr/bin/lycia_music' 2>/dev/null || true

    # 最多等待 5 秒让进程优雅退出（保存数据库、释放音频句柄）。
    i=0
    while [ "$i" -lt 10 ]; do
        pgrep -x lycia_music >/dev/null 2>&1 || break
        sleep 0.5
        i=$((i + 1))
    done

    # 仍未退出则强制终止，避免卸载后的僵尸托盘项。
    if pgrep -x lycia_music >/dev/null 2>&1; then
        pkill -KILL -x lycia_music 2>/dev/null || true
    fi
fi

exit 0
