import { computed, onMounted, onUnmounted, ref } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { getCurrentWindow, type Window } from '@tauri-apps/api/window';
import {
  isPermissionGranted,
  requestPermission,
  sendNotification,
} from '@tauri-apps/plugin-notification';

import { setLibrarySongPathCacheLowPower } from '../caches/librarySongPathCache';

export interface MainWindowRenderingSnapshot {
  documentHidden: boolean;
  windowFocused: boolean;
  windowVisible: boolean;
  windowMinimized: boolean;
}

const defaultSnapshot = (): MainWindowRenderingSnapshot => ({
  documentHidden: typeof document !== 'undefined' ? document.hidden : false,
  windowFocused: true,
  windowVisible: true,
  windowMinimized: false,
});

const mainWindowRenderingSnapshot = ref<MainWindowRenderingSnapshot>(defaultSnapshot());

export function resolveMainWindowLowPower(snapshot: MainWindowRenderingSnapshot) {
  return snapshot.documentHidden
    || !snapshot.windowVisible
    || snapshot.windowMinimized;
}

export function setMainWindowRenderingSnapshot(patch: Partial<MainWindowRenderingSnapshot>) {
  mainWindowRenderingSnapshot.value = {
    ...mainWindowRenderingSnapshot.value,
    ...patch,
  };
  setLibrarySongPathCacheLowPower(resolveMainWindowLowPower(mainWindowRenderingSnapshot.value));
}

export function setMainWindowFocusState(windowFocused: boolean) {
  setMainWindowRenderingSnapshot(windowFocused
    ? {
        documentHidden: false,
        windowFocused: true,
        windowVisible: true,
        windowMinimized: false,
      }
    : { windowFocused: false });
}

const isMainWindowLowPower = computed(() => resolveMainWindowLowPower(mainWindowRenderingSnapshot.value));

export function useRenderingPower() {
  return {
    mainWindowRenderingSnapshot,
    isMainWindowLowPower,
  };
}

let trayAvailabilityCache: { value: boolean; expiresAt: number } | null = null;

/**
 * 当前会话是否支持系统托盘。
 *
 * Linux 上托盘依赖 `org.kde.StatusNotifierWatcher`：KDE Plasma / Cinnamon /
 * XFCE 原生提供，GNOME 需要安装 AppIndicator 扩展。结果做 30 秒缓存（探测
 * 是一次 D-Bus 往返，且用户中途装扩展的场景以分钟计），查询失败时保守地
 * 视为可用以维持既有行为。
 */
export async function isTrayAvailable(): Promise<boolean> {
  const now = Date.now();
  if (trayAvailabilityCache && trayAvailabilityCache.expiresAt > now) {
    return trayAvailabilityCache.value;
  }

  try {
    const value = await invoke<boolean>('is_tray_available');
    trayAvailabilityCache = { value, expiresAt: now + 30_000 };
    return value;
  } catch {
    return true;
  }
}

let noTraySupportNotified = false;

async function warnNoTraySupport() {
  if (noTraySupportNotified) return;
  noTraySupportNotified = true;
  try {
    let permissionGranted = await isPermissionGranted();
    if (!permissionGranted) {
      permissionGranted = (await requestPermission()) === 'granted';
    }
    if (permissionGranted) {
      sendNotification({
        title: 'Lycia Player',
        body: '当前桌面环境不支持系统托盘（GNOME 需启用 AppIndicator 扩展），已直接退出。',
      });
    }
  } catch {}
}

export async function hideMainWindowToTray(appWindow: Window = getCurrentWindow()) {
  // 托盘不可用的环境里"隐藏到托盘"会制造一个既看不见又无法退出的后台
  // 进程（任务栏没有窗口、托盘没有图标）。降级为真正退出，避免应用失控。
  if (!(await isTrayAvailable())) {
    await warnNoTraySupport();
    await appWindow.destroy();
    return;
  }

  await appWindow.hide();
  setMainWindowRenderingSnapshot({
    documentHidden: true,
    windowFocused: false,
    windowVisible: false,
    windowMinimized: false,
  });
}

export function useMainWindowRenderingPower() {
  const appWindow = getCurrentWindow();
  const unlisteners: Array<() => void> = [];

  const syncDocumentVisibility = () => {
    setMainWindowRenderingSnapshot({ documentHidden: document.hidden });
  };

  const syncWindowState = async () => {
    try {
      const [windowFocused, windowVisible, windowMinimized] = await Promise.all([
        appWindow.isFocused(),
        appWindow.isVisible(),
        appWindow.isMinimized(),
      ]);

      setMainWindowRenderingSnapshot({
        windowFocused,
        windowVisible,
        windowMinimized,
      });
    } catch {}
  };

  onMounted(async () => {
    syncDocumentVisibility();
    await syncWindowState();

    document.addEventListener('visibilitychange', syncDocumentVisibility);
    window.addEventListener('focus', syncWindowState);
    window.addEventListener('blur', syncWindowState);

    unlisteners.push(await appWindow.onFocusChanged(({ payload }) => {
      // A focused native window is necessarily visible. Clear the explicit
      // documentHidden flag set by close-to-tray even when WebView2 omits the
      // matching visibilitychange event during restoration.
      setMainWindowFocusState(payload);
      void syncWindowState();
    }));
    unlisteners.push(await appWindow.onResized(() => {
      void syncWindowState();
    }));
  });

  onUnmounted(() => {
    document.removeEventListener('visibilitychange', syncDocumentVisibility);
    window.removeEventListener('focus', syncWindowState);
    window.removeEventListener('blur', syncWindowState);
    unlisteners.splice(0).forEach((unlisten) => unlisten());
  });

  return useRenderingPower();
}
