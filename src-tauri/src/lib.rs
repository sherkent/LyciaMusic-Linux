mod app_runtime;
mod custom_background;
mod custom_fonts;
mod database;
pub mod error;
mod foreground_window;
mod highlights;
mod music;
mod player;
mod remote;
mod statistics;
mod system_fonts;
mod taskbar;
mod toolbox;
mod window_boundary;
mod window_fullscreen;
mod window_material;
mod window_theme;
mod window_z_order;

use app_runtime::{
    consume_pending_open_paths, exit_app, handle_single_instance, is_tray_available, setup_app,
};
use custom_background::prepare_custom_background_image;
use custom_fonts::{import_lyrics_font, read_lyrics_font_data_url};
use database::clear_all_app_data;
use foreground_window::get_foreground_fullscreen_state;
use highlights::{
    add_song_highlight_marker, delete_song_highlight_marker, get_song_highlight_markers,
    set_song_highlight_marker_position, set_song_highlight_primary, undo_song_highlight_add,
};
use music::{
    add_library_folder, add_sidebar_folder, batch_move_music_files, clear_cover_cache,
    create_folder, delete_folder, delete_music_file, get_folder_children, get_folder_first_song,
    get_library_album_catalog, get_library_album_catalog_by_artist, get_library_artist_catalog,
    get_library_folders, get_library_hierarchy, get_library_song_labels_for_all_view,
    get_library_song_page, get_library_song_path_page_for_all_view,
    get_library_song_paths_by_album, get_library_song_paths_by_artist,
    get_library_song_paths_cached, get_library_song_paths_for_all_view,
    get_library_song_paths_for_folder_view, get_library_songs_by_paths, get_library_songs_cached,
    get_search_index_batch, get_search_index_status, get_sidebar_folders, get_sidebar_hierarchy,
    get_song_cover, get_song_cover_thumbnail, get_song_detail, get_song_lyrics,
    get_song_lyrics_for_edit, get_song_lyrics_payload, get_song_runtime_metadata, is_directory,
    move_file_to_folder, move_music_file, parse_audio_files, remove_library_folder,
    remove_sidebar_folder, save_artist_avatar, save_song_info, save_song_lyrics,
    scan_folder_as_playlists, scan_library, scan_music_folder, show_in_folder,
    upsert_search_index_batch,
};
use player::{
    get_audio_visualizer_samples, get_current_output_device, get_output_devices,
    get_playback_progress, get_track_loudness_info, pause_audio, play_audio, resume_audio,
    seek_audio, set_audio_output_mode, set_audio_visualizer_enabled, set_equalizer_settings,
    set_output_device, set_volume, stop_audio, update_loudness_settings, update_playback_metadata,
};
use remote::{
    add_remote_source, clear_remote_cache, get_remote_cache_usage, get_remote_sources,
    list_remote_directory, precache_remote_song, remove_remote_source, sync_remote_source,
    test_remote_source, update_remote_source,
};
use statistics::{
    add_to_history, clear_recent_history, export_statistics_file, get_behavior_stats,
    get_favorite_album_catalog, get_favorite_artist_catalog, get_favorite_song_paths_view,
    get_format_distribution, get_library_stats, get_quality_distribution, get_recent_album_catalog,
    get_recent_history, get_recent_playlist_catalog, get_recent_song_paths_view,
    import_recent_history, import_statistics_file, preview_statistics_import, record_play,
    remove_from_recent_history, remove_songs_from_history_and_statistics,
};
use system_fonts::get_system_fonts;
use taskbar::{
    get_taskbar_tray_geometry, install_taskbar_zorder_guard, refresh_taskbar_window_topmost,
    setup_taskbar_window, uninstall_taskbar_zorder_guard,
};
use tauri::Manager;
use toolbox::{
    apply_rename, check_update_by_rust, download_update_file, file_exists, open_external_program,
    preview_rename, refresh_folder_songs, run_installer, set_gpu_acceleration,
};

#[cfg(target_os = "windows")]
use toolbox::{append_webview2_browser_arg, should_disable_gpu_for_startup};
use window_boundary::set_retain_material_on_unfocus;
use window_fullscreen::set_immersive_fullscreen;
use window_material::get_window_material_capabilities;
use window_theme::set_dark_mode_for_window;
use window_z_order::{refresh_current_window_topmost, start_topmost_guard, stop_topmost_guard};

#[cfg(target_os = "linux")]
fn set_glib_identity(name: &str) {
    use std::ffi::CString;

    extern "C" {
        fn g_set_prgname(prgname: *const std::os::raw::c_char);
        fn g_set_application_name(application_name: *const std::os::raw::c_char);
    }

    // GLib 在首次调用后持有字符串副本；任何失败都只是回退到默认身份，
    // 不影响启动。
    if let Ok(c_name) = CString::new(name) {
        unsafe {
            g_set_prgname(c_name.as_ptr());
            g_set_application_name(c_name.as_ptr());
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    #[cfg(target_os = "windows")]
    {
        if should_disable_gpu_for_startup() {
            append_webview2_browser_arg("--disable-gpu");
        }
    }

    #[cfg(target_os = "linux")]
    {
        // WebKitGTK 在软件渲染环境（WSLg、无 GPU 虚拟机）下的渲染问题实测：
        // 1. DMABUF 渲染器（默认开启）：交互触发整页重光栅化，UI 冻结数秒，
        //    且界面更新闪屏；
        // 2. 合成器 + 禁 DMABUF：缓冲提交损坏，出现大块透明/消失区域；
        // 3. 非合成模式 + 不透明窗口：渲染最稳定——配合
        //    tauri.linux.conf.json 将窗口 transparent 关闭，GTK 恢复内置
        //    双缓冲，重绘不再闪烁；无合成也消除了冻结与缓冲损坏问题。
        // 若外部已显式设置相关环境变量则尊重用户配置，便于按环境覆盖。
        if std::env::var_os("WEBKIT_DISABLE_COMPOSITING_MODE").is_none()
            && std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none()
        {
            std::env::set_var("WEBKIT_DISABLE_COMPOSITING_MODE", "1");
        }

        // 对齐窗口身份与桌面入口：GTK 在 Wayland 上用 prgname 作为 app_id、
        // 在 X11 上用它推导 WM_CLASS。不设置时默认是二进制名（lycia_music），
        // 与桌面文件 "Lycia Player.desktop" 无法匹配，导致 GNOME/KDE 的任务
        // 栏与 Dock 无法把窗口关联到应用图标。必须在 gtk_init 之前设置
        // （Tauri 在 Builder 内部才会初始化 GTK），直接走 libglib FFI 避免
        // 额外引入 gtk-rs 依赖。
        set_glib_identity("Lycia Player");
    }

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            handle_single_instance(app, argv);
        }))
        .on_window_event(|window, event| {
            if window.label() == "main" {
                if let tauri::WindowEvent::Destroyed = event {
                    window.app_handle().exit(0);
                }
            }
        })
        .plugin(
            tauri_plugin_window_state::Builder::default()
                .with_denylist(&[
                    "desktop-lyrics",
                    "taskbar-player",
                    "tray-menu",
                ])
                .build(),
        )
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| setup_app(app))
        .invoke_handler(tauri::generate_handler![
            scan_music_folder,
            parse_audio_files,
            scan_folder_as_playlists,
            get_song_cover_thumbnail,
            get_song_cover,
            clear_cover_cache,
            prepare_custom_background_image,
            get_song_lyrics,
            get_song_lyrics_payload,
            get_song_lyrics_for_edit,
            save_song_lyrics,
            save_song_info,
            get_song_detail,
            get_song_runtime_metadata,
            get_song_highlight_markers,
            add_song_highlight_marker,
            set_song_highlight_marker_position,
            set_song_highlight_primary,
            delete_song_highlight_marker,
            undo_song_highlight_add,
            batch_move_music_files,
            move_music_file,
            show_in_folder,
            delete_music_file,
            play_audio,
            update_playback_metadata,
            pause_audio,
            stop_audio,
            resume_audio,
            seek_audio,
            set_volume,
            get_playback_progress,
            get_audio_visualizer_samples,
            set_audio_visualizer_enabled,
            get_track_loudness_info,
            update_loudness_settings,
            set_equalizer_settings,
            preview_rename,
            apply_rename,
            get_output_devices,
            get_current_output_device,
            set_output_device,
            set_audio_output_mode,
            get_library_folders,
            is_directory,
            save_artist_avatar,
            add_library_folder,
            remove_library_folder,
            get_library_songs_cached,
            get_library_song_paths_cached,
            get_library_song_page,
            get_library_song_path_page_for_all_view,
            get_library_songs_by_paths,
            get_library_song_labels_for_all_view,
            get_search_index_status,
            get_search_index_batch,
            upsert_search_index_batch,
            get_library_artist_catalog,
            get_library_album_catalog,
            get_library_album_catalog_by_artist,
            get_library_song_paths_by_artist,
            get_library_song_paths_by_album,
            get_library_song_paths_for_all_view,
            get_library_song_paths_for_folder_view,
            get_remote_sources,
            test_remote_source,
            add_remote_source,
            update_remote_source,
            remove_remote_source,
            sync_remote_source,
            precache_remote_song,
            get_remote_cache_usage,
            clear_remote_cache,
            list_remote_directory,
            scan_library,
            get_library_hierarchy,
            get_folder_children,
            // Deprecated compatibility commands for legacy sidebar_folders.
            get_sidebar_folders,
            add_sidebar_folder,
            remove_sidebar_folder,
            get_sidebar_hierarchy,
            create_folder,
            delete_folder,
            move_file_to_folder,
            get_folder_first_song,
            get_library_stats,
            add_to_history,
            record_play,
            get_recent_history,
            get_favorite_artist_catalog,
            get_favorite_album_catalog,
            get_favorite_song_paths_view,
            get_recent_album_catalog,
            get_recent_song_paths_view,
            get_recent_playlist_catalog,
            import_recent_history,
            export_statistics_file,
            preview_statistics_import,
            import_statistics_file,
            remove_from_recent_history,
            remove_songs_from_history_and_statistics,
            clear_recent_history,
            get_behavior_stats,
            get_quality_distribution,
            get_format_distribution,
            clear_all_app_data,
            open_external_program,
            file_exists,
            refresh_folder_songs,
            set_retain_material_on_unfocus,
            get_window_material_capabilities,
            get_foreground_fullscreen_state,
            set_immersive_fullscreen,
            set_dark_mode_for_window,
            refresh_current_window_topmost,
            start_topmost_guard,
            stop_topmost_guard,
            consume_pending_open_paths,
            is_tray_available,
            get_system_fonts,
            import_lyrics_font,
            read_lyrics_font_data_url,
            setup_taskbar_window,
            get_taskbar_tray_geometry,
            install_taskbar_zorder_guard,
            refresh_taskbar_window_topmost,
            uninstall_taskbar_zorder_guard,
            exit_app,
            set_gpu_acceleration,
            check_update_by_rust,
            download_update_file,
            run_installer
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
