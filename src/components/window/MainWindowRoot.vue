<script setup lang="ts">
import '@applemusic-like-lyrics/core/style.css';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { onMounted, onUnmounted } from 'vue';

import { useImportedLyricsFonts } from '../../composables/useImportedLyricsFonts';
import { hideMainWindowToTray } from '../../composables/renderingPower';
import { useSettings } from '../../features/settings/useSettings';
import MainShell from '../layout/MainShell.vue';

const appWindow = getCurrentWindow();
const { settings } = useSettings();
let unlistenCloseRequested: (() => void) | null = null;

useImportedLyricsFonts();

onMounted(async () => {
  unlistenCloseRequested = await appWindow.onCloseRequested(async event => {
    if (settings.value.closeToTray) {
      event.preventDefault();
      // 统一走 hideMainWindowToTray：托盘不可用的桌面环境（如未启用
      // AppIndicator 扩展的 GNOME）会降级为真正退出，避免留下无法恢复
      // 的后台进程。
      await hideMainWindowToTray(appWindow);
    }
  });
});

onUnmounted(() => {
  unlistenCloseRequested?.();
  unlistenCloseRequested = null;
});
</script>

<template>
  <MainShell />
</template>
