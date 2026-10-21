use crate::database::DbState;
use crate::music::{
    run_cache_cleanup, FullCoverImageConcurrencyLimit, ThumbnailImageConcurrencyLimit,
    FULL_COVER_IMAGE_CONCURRENCY_LIMIT, THUMBNAIL_IMAGE_CONCURRENCY_LIMIT,
};
use crate::player::init_player;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
#[cfg(not(target_os = "linux"))]
use tauri::tray::{MouseButton, MouseButtonState, TrayIconEvent};
use tauri::tray::TrayIconBuilder;
use tauri::{Emitter, Manager};
use tokio::sync::Semaphore;

// 自绘托盘菜单（Windows）与 Linux 原生托盘菜单共用的动作通道，
// 主窗口前端的 useTrayMenuEvents 监听此事件并复用 handleTrayMenuAction。
const APP_TRAY_MENU_EVENT: &str = "app:tray-menu";
#[cfg(not(target_os = "linux"))]
const APP_TRAY_MENU_OPEN_EVENT: &str = "app:tray-menu-open";
const MAIN_WINDOW_LABEL: &str = "main";

#[derive(serde::Serialize, Clone, Copy)]
#[cfg(not(target_os = "linux"))]
#[serde(rename_all = "camelCase")]
struct TrayMenuOpenPayload {
    x: f64,
    y: f64,
}

#[derive(Default)]
pub(crate) struct PendingOpenPaths(pub(crate) Mutex<Vec<String>>);

fn append_unique_paths(target: &mut Vec<String>, incoming: impl IntoIterator<Item = String>) {
    let mut seen = target.iter().cloned().collect::<HashSet<_>>();

    for path in incoming {
        if seen.insert(path.clone()) {
            target.push(path);
        }
    }
}

fn collect_existing_open_paths(
    args: impl IntoIterator<Item = String>,
    current_exe: Option<&Path>,
) -> Vec<String> {
    let mut paths = Vec::new();
    let mut seen = HashSet::new();

    for arg in args {
        let trimmed = arg.trim();
        if trimmed.is_empty() {
            continue;
        }

        let normalized = crate::music::utils::normalize_path(trimmed);
        if normalized.is_empty() {
            continue;
        }

        let candidate = PathBuf::from(&normalized);
        if !candidate.exists() {
            continue;
        }

        if current_exe.is_some_and(|exe| exe == candidate.as_path()) {
            continue;
        }

        if seen.insert(normalized.clone()) {
            paths.push(normalized);
        }
    }

    paths
}

fn queue_open_paths<R: tauri::Runtime>(app: &tauri::AppHandle<R>, paths: Vec<String>) {
    if paths.is_empty() {
        return;
    }

    if let Some(state) = app.try_state::<PendingOpenPaths>() {
        if let Ok(mut pending_paths) = state.0.lock() {
            append_unique_paths(&mut pending_paths, paths);
        }
    }
}

fn reveal_main_window<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

// 自绘托盘菜单仅在有原生点击事件的平台（Windows/macOS）使用；Linux 的
// libappindicator 不产生点击事件，走下方原生菜单方案。
#[cfg(not(target_os = "linux"))]
fn emit_tray_menu_open<R: tauri::Runtime>(app: &tauri::AppHandle<R>, x: f64, y: f64) {
    let _ = app.emit(APP_TRAY_MENU_OPEN_EVENT, TrayMenuOpenPayload { x, y });
}

fn install_window_boundary<R: tauri::Runtime>(app: &tauri::App<R>) {
    #[cfg(target_os = "windows")]
    {
        if let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) {
            use raw_window_handle::HasWindowHandle;

            if let Ok(handle) = window.as_ref().window().window_handle() {
                if let raw_window_handle::RawWindowHandle::Win32(win32) = handle.as_raw() {
                    crate::window_boundary::install_boundary_subclass(win32.hwnd.get() as isize);
                }
            }
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn build_tray<R: tauri::Runtime>(app: &tauri::App<R>) -> tauri::Result<()> {
    let _tray = TrayIconBuilder::with_id("tray")
        .icon(app.default_window_icon().unwrap().clone())
        .show_menu_on_left_click(false)
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                reveal_main_window(&tray.app_handle());
                return;
            }

            if let TrayIconEvent::Click {
                button: MouseButton::Right,
                button_state: MouseButtonState::Up,
                position,
                ..
            } = event
            {
                emit_tray_menu_open(&tray.app_handle(), position.x, position.y);
            }
        })
        .build(app.handle())?;

    Ok(())
}

/// Linux 托盘兼容层。
///
/// tray-icon crate 在 Linux 上基于 libappindicator/StatusNotifierItem 实现，
/// 它不提供任何点击事件（`TrayIconEvent` 在 Linux 上永远不会触发），tooltip
/// 也是空操作。这意味着上面 Windows 风格的"左键唤起主窗口、右键唤起自绘
/// 菜单"交互在 KDE Plasma / GNOME (AppIndicator 扩展) / Cinnamon / XFCE 上
/// 全部失效，托盘图标会变成一个无法交互的死图标。
///
/// libappindicator 唯一可靠的交互方式是原生菜单（dbus 菜单，各桌面通过
/// StatusNotifierItem 协议渲染）。这里为 Linux 挂载原生菜单：左键/右键都会
/// 弹出菜单，播放控制项通过既有的 `app:tray-menu` 事件转发给主窗口前端，
/// 与 Windows 自绘托盘菜单共用同一套动作处理逻辑（features/tray/actions.ts）。
#[cfg(target_os = "linux")]
fn build_tray<R: tauri::Runtime>(app: &tauri::App<R>) -> tauri::Result<()> {
    use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};

    let show_main = MenuItem::with_id(app, "tray-show-main", "显示主窗口", true, None::<&str>)?;
    let prev_song = MenuItem::with_id(app, "tray-prev-song", "上一首", true, None::<&str>)?;
    let toggle_play =
        MenuItem::with_id(app, "tray-toggle-play", "播放 / 暂停", true, None::<&str>)?;
    let next_song = MenuItem::with_id(app, "tray-next-song", "下一首", true, None::<&str>)?;
    let mode_list = MenuItem::with_id(app, "tray-mode-list", "列表循环", true, None::<&str>)?;
    let mode_single =
        MenuItem::with_id(app, "tray-mode-single", "单曲循环", true, None::<&str>)?;
    let mode_shuffle = MenuItem::with_id(app, "tray-mode-shuffle", "随机播放", true, None::<&str>)?;
    let desktop_lyrics =
        MenuItem::with_id(app, "tray-desktop-lyrics", "打开桌面歌词", true, None::<&str>)?;
    let open_settings = MenuItem::with_id(app, "tray-open-settings", "设置", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "tray-quit", "退出", true, None::<&str>)?;

    let menu = Menu::with_items(
        app,
        &[
            &show_main,
            &PredefinedMenuItem::separator(app)?,
            &prev_song,
            &toggle_play,
            &next_song,
            &PredefinedMenuItem::separator(app)?,
            &mode_list,
            &mode_single,
            &mode_shuffle,
            &PredefinedMenuItem::separator(app)?,
            &desktop_lyrics,
            &open_settings,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;

    let _tray = TrayIconBuilder::with_id("tray")
        .icon(app.default_window_icon().unwrap().clone())
        .menu(&menu)
        .show_menu_on_left_click(true)
        .build(app.handle())?;

    Ok(())
}

/// 处理 Linux 托盘原生菜单的点击事件。
///
/// 与窗口生命周期相关的动作（唤起主窗口、退出）直接在 Rust 侧完成，
/// 不依赖主窗口 webview 是否已经就绪；播放控制类动作转发给主窗口前端
/// 复用 `handleTrayMenuAction`。
#[cfg(target_os = "linux")]
fn handle_tray_menu_event<R: tauri::Runtime>(app: &tauri::AppHandle<R>, menu_id: &str) {
    let forward = |action: &str| {
        let _ = app.emit_to(MAIN_WINDOW_LABEL, APP_TRAY_MENU_EVENT, action);
    };

    match menu_id {
        "tray-show-main" => reveal_main_window(app),
        "tray-quit" => app.exit(0),
        "tray-prev-song" => forward("prev-song"),
        "tray-toggle-play" => forward("toggle-play"),
        "tray-next-song" => forward("next-song"),
        "tray-mode-list" => forward("play-mode-list-loop"),
        "tray-mode-single" => forward("play-mode-single-loop"),
        "tray-mode-shuffle" => forward("play-mode-shuffle"),
        "tray-desktop-lyrics" => forward("open-desktop-lyrics"),
        "tray-open-settings" => forward("open-settings"),
        _ => {}
    }
}

pub(crate) fn handle_single_instance<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    argv: Vec<String>,
) {
    let current_exe = std::env::current_exe().ok();
    let open_paths = collect_existing_open_paths(argv, current_exe.as_deref());
    queue_open_paths(app, open_paths);
    let _ = app.emit("app:open-paths", ());
    reveal_main_window(app);
}

pub(crate) fn setup_app(
    app: &mut tauri::App<tauri::Wry>,
) -> Result<(), Box<dyn std::error::Error>> {
    app.manage(PendingOpenPaths::default());

    let db_state = DbState::new(app.handle())?;
    app.manage(db_state);

    let player_state = init_player(app.handle());
    app.manage(player_state);

    app.manage(ThumbnailImageConcurrencyLimit(Semaphore::new(
        THUMBNAIL_IMAGE_CONCURRENCY_LIMIT,
    )));
    app.manage(FullCoverImageConcurrencyLimit(Semaphore::new(
        FULL_COVER_IMAGE_CONCURRENCY_LIMIT,
    )));
    run_cache_cleanup(app.handle());

    let current_exe = std::env::current_exe().ok();
    let initial_open_paths =
        collect_existing_open_paths(std::env::args().skip(1), current_exe.as_deref());
    queue_open_paths(app.handle(), initial_open_paths);

    install_window_boundary(app);
    if let Err(error) = build_tray(app) {
        eprintln!("Warning: failed to build tray icon: {:?}", error);
    }

    #[cfg(target_os = "linux")]
    app.on_menu_event(|app, event| {
        handle_tray_menu_event(app, event.id().as_ref());
    });

    Ok(())
}

#[tauri::command]
pub(crate) fn consume_pending_open_paths(
    state: tauri::State<PendingOpenPaths>,
) -> Result<Vec<String>, String> {
    let mut pending_paths = state.0.lock().map_err(|error| error.to_string())?;
    Ok(std::mem::take(&mut *pending_paths))
}

/// 检测当前会话是否支持系统托盘（StatusNotifierItem 协议）。
///
/// Linux 上托盘经由 `org.kde.StatusNotifierWatcher` 这个 D-Bus 服务仲裁：
/// - KDE Plasma / Cinnamon / XFCE 面板原生注册该服务；
/// - GNOME 需要安装 AppIndicator/KStatusNotifierItem 扩展（GNOME 43+ 已移除
///   传统 XEmbed 托盘）。
///
/// 若查询不到 Watcher，说明用户环境无法显示托盘图标，此时"关闭到托盘"
/// 会让应用变成一个无法恢复、也无法退出的后台进程——前端需要据此把
/// 关闭动作降级为真正的退出。
#[cfg(target_os = "linux")]
#[tauri::command]
pub(crate) async fn is_tray_available() -> Result<bool, String> {
    // tray-icon 的 Linux 后端依赖 libappindicator（其内部同样走 dbus 栈），
    // 这里复用 zbus 栈查询会话总线上是否存在 StatusNotifierWatcher。
    use zbus::fdo::DBusProxy;

    let connection = zbus::Connection::session()
        .await
        .map_err(|error| format!("连接会话总线失败: {error}"))?;
    let proxy = DBusProxy::new(&connection)
        .await
        .map_err(|error| format!("创建DBus代理失败: {error}"))?;

    let names = proxy
        .list_names()
        .await
        .map_err(|error| format!("查询总线名称失败: {error}"))?;

    Ok(names.iter().any(|name| name.as_str() == "org.kde.StatusNotifierWatcher"))
}

#[cfg(not(target_os = "linux"))]
#[tauri::command]
pub(crate) async fn is_tray_available() -> Result<bool, String> {
    // Windows 原生托盘始终可用。
    Ok(true)
}

#[tauri::command]
pub(crate) fn exit_app(app: tauri::AppHandle) {
    app.exit(0);
}
