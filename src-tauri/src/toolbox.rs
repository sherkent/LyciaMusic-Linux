use crate::music::tags::{extract_text_metadata, read_tagged_file_from_path};
use crate::music::utils::is_supported_library_extension;
use lofty::prelude::*;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

#[derive(Debug, Serialize, Deserialize)]
pub struct RenameConfig {
    pub mode: String,     // "tags", "rules", "auto"
    pub template: String, // e.g. "{artist} - {title}"
    pub remove_track_prefix: bool,
    pub remove_source_prefix: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RenamePreview {
    pub original_path: String,
    pub original_name: String,
    pub new_name: String,
    pub status: String, // "tags" (success via tags), "rules" (cleaned via rules), "skipped" (no change/error)
    pub error: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RenameOperation {
    pub original_path: String,
    pub new_name: String,
}

fn sanitize_filename(name: &str) -> String {
    let invalid_chars = ['<', '>', ':', '"', '/', '\\', '|', '?', '*'];
    let mut sanitized = String::new();
    for c in name.chars() {
        if invalid_chars.contains(&c) {
            sanitized.push('_');
        } else {
            sanitized.push(c);
        }
    }
    sanitized.trim().to_string()
}

fn process_file(path: &Path, config: &RenameConfig) -> RenamePreview {
    let original_name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let original_path_str = path.to_string_lossy().to_string();
    let ext = path
        .extension()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    // Mode A: Standardize via Tags
    if config.mode == "tags" || config.mode == "auto" {
        if let Ok(tagged_file) = read_tagged_file_from_path(path) {
            let metadata = extract_text_metadata(&tagged_file);
            let title = metadata.title.unwrap_or_default();
            let artist = metadata.artist.unwrap_or_default();
            let album = metadata.album.unwrap_or_default();

            let year = tagged_file
                .primary_tag()
                .and_then(|tag| tag.year())
                .map(|y| y.to_string())
                .unwrap_or_default();
            let track = tagged_file
                .primary_tag()
                .and_then(|tag| tag.track())
                .map(|t| format!("{:02}", t))
                .unwrap_or_default();

            if !title.is_empty() {
                let mut new_name_base = config.template.clone();
                new_name_base = new_name_base.replace("{title}", &title);
                new_name_base = new_name_base.replace("{artist}", &artist);
                new_name_base = new_name_base.replace("{album}", &album);
                new_name_base = new_name_base.replace("{year}", &year);
                new_name_base = new_name_base.replace("{track}", &track);

                let new_name = format!("{}.{}", sanitize_filename(&new_name_base), ext);

                if new_name != original_name {
                    return RenamePreview {
                        original_path: original_path_str,
                        original_name,
                        new_name,
                        status: "tags".to_string(),
                        error: None,
                    };
                } else if config.mode == "tags" {
                    return RenamePreview {
                        original_path: original_path_str,
                        original_name: original_name.clone(),
                        new_name: original_name,
                        status: "skipped".to_string(),
                        error: Some("Already named correctly".to_string()),
                    };
                }
            }
        }

        // If mode is "tags" and we failed, return skipped
        if config.mode == "tags" {
            return RenamePreview {
                original_path: original_path_str,
                original_name: original_name.clone(),
                new_name: original_name,
                status: "skipped".to_string(),
                error: Some("Missing tags".to_string()),
            };
        }
    }

    // Mode B: Clean via Rules (or Auto fallback)
    if config.mode == "rules" || config.mode == "auto" {
        let mut cleaned_name = original_name.clone();

        // Apply regex rules only to the stem (filename without extension)
        if let Some(stem) = path.file_stem() {
            let mut stem_str = stem.to_string_lossy().to_string();

            if config.remove_track_prefix {
                let re = Regex::new(r"^\d+[\.\-\s]+").unwrap();
                stem_str = re.replace(&stem_str, "").to_string();
            }

            if config.remove_source_prefix {
                let re = Regex::new(r"^\s*\[.*?\]\s*").unwrap();
                stem_str = re.replace(&stem_str, "").to_string();
            }

            cleaned_name = format!("{}.{}", stem_str.trim(), ext);
        }

        if cleaned_name != original_name {
            return RenamePreview {
                original_path: original_path_str,
                original_name,
                new_name: cleaned_name,
                status: "rules".to_string(),
                error: None,
            };
        }
    }

    // Fallback: Skipped
    RenamePreview {
        original_path: original_path_str,
        original_name: original_name.clone(),
        new_name: original_name,
        status: "skipped".to_string(),
        error: Some("No rules matched or missing tags".to_string()),
    }
}

#[tauri::command]
pub fn preview_rename(
    root_path: String,
    config: RenameConfig,
) -> Result<Vec<RenamePreview>, String> {
    let mut results = Vec::new();

    for entry in WalkDir::new(root_path)
        .max_depth(1)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let path = entry.path();
        if path.is_file() {
            if let Some(ext) = path.extension() {
                let ext = ext.to_string_lossy().to_lowercase();
                if is_supported_library_extension(&ext) {
                    results.push(process_file(path, &config));
                }
            }
        }
    }

    // Sort logic: changed files first
    results.sort_by(|a, b| {
        let a_changed = a.status != "skipped";
        let b_changed = b.status != "skipped";
        if a_changed && !b_changed {
            std::cmp::Ordering::Less
        } else if !a_changed && b_changed {
            std::cmp::Ordering::Greater
        } else {
            a.original_name.cmp(&b.original_name)
        }
    });

    Ok(results)
}

#[tauri::command]
pub fn apply_rename(operations: Vec<RenameOperation>) -> Result<u32, String> {
    let mut success_count = 0;

    for op in operations {
        let src = PathBuf::from(&op.original_path);
        if let Some(parent) = src.parent() {
            let dest = parent.join(&op.new_name);
            if fs::rename(&src, &dest).is_ok() {
                success_count += 1;
            } else {
                eprintln!("Failed to rename {:?} to {:?}", src, dest);
            }
        }
    }

    Ok(success_count)
}

#[tauri::command]
pub fn open_external_program(path: String, args: Vec<String>) -> Result<(), String> {
    use std::process::Command;

    let mut cmd = Command::new(&path);
    for arg in args {
        cmd.arg(arg);
    }

    cmd.spawn()
        .map_err(|e| format!("Failed to launch program: {}", e))?;

    Ok(())
}

#[tauri::command]
pub fn refresh_folder_songs(
    folder_path: String,
    minimum_duration_seconds: Option<u32>,
    db_state: tauri::State<'_, crate::database::DbState>,
) -> Result<Vec<crate::music::types::Song>, String> {
    // 复用现有的扫描逻辑
    crate::music::scanner::scan_single_directory_internal(
        folder_path,
        db_state.conn.clone(),
        None,
        1,
        1,
        crate::music::scanner::ScanOptions::from_minimum_duration_seconds(minimum_duration_seconds),
    )
}

#[tauri::command]
pub fn file_exists(path: String) -> bool {
    std::path::Path::new(&path).is_file()
}

const APP_IDENTIFIER: &str = "com.lover.lyciaplayer";
const GPU_CONFIG_FILE: &str = "gpu_config.json";

#[derive(Debug, Serialize, Deserialize)]
struct GpuConfig {
    gpu_acceleration: bool,
}

#[cfg(target_os = "windows")]
pub fn gpu_config_path() -> Result<PathBuf, String> {
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .map(|dir| dir.join(APP_IDENTIFIER).join(GPU_CONFIG_FILE))
        .ok_or_else(|| "APPDATA environment variable not found".to_string())
}

#[cfg(target_os = "windows")]
pub fn should_disable_gpu_for_startup() -> bool {
    let Ok(path) = gpu_config_path() else {
        return false;
    };

    if !path.exists() {
        return false;
    }

    let Ok(content) = fs::read_to_string(path) else {
        return false;
    };

    match serde_json::from_str::<GpuConfig>(&content) {
        Ok(config) => !config.gpu_acceleration,
        Err(_) => false,
    }
}

#[cfg(target_os = "windows")]
pub fn append_webview2_browser_arg(arg: &str) {
    const KEY: &str = "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS";

    let current = std::env::var(KEY).unwrap_or_default();

    if current.split_whitespace().any(|item| item == arg) {
        return;
    }

    let next = if current.trim().is_empty() {
        arg.to_string()
    } else {
        format!("{} {}", current.trim(), arg)
    };

    std::env::set_var(KEY, next);
}

#[tauri::command]
pub fn set_gpu_acceleration(app_handle: tauri::AppHandle, enabled: bool) -> Result<(), String> {
    #[cfg(not(target_os = "windows"))]
    use tauri::Manager;

    #[cfg(target_os = "windows")]
    let path = {
        let _ = app_handle;
        gpu_config_path()?
    };

    #[cfg(not(target_os = "windows"))]
    let path = app_handle
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join(GPU_CONFIG_FILE);

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    let config = GpuConfig {
        gpu_acceleration: enabled,
    };

    let content = serde_json::to_string_pretty(&config).map_err(|e| e.to_string())?;

    std::fs::write(path, content).map_err(|e| e.to_string())?;

    Ok(())
}

use std::time::Duration;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UpdateSource {
    Official,
    Github,
}

#[tauri::command]
pub async fn check_update_by_rust(source: UpdateSource) -> Result<String, String> {
    let url = match source {
        UpdateSource::Official | UpdateSource::Github => {
            "https://api.github.com/repos/sherkent/LyciaMusic-Linux/releases/latest"
        }
    };

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .user_agent("LyciaPlayer-Updater")
        .build()
        .map_err(|e| format!("创建更新请求失败: {e}"))?;

    client
        .get(url)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| format!("请求更新接口失败: {e}"))?
        .error_for_status()
        .map_err(|e| format!("更新接口返回错误状态: {e}"))?
        .text()
        .await
        .map_err(|e| format!("读取更新数据失败: {e}"))
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct DownloadProgress {
    pub progress: f64,
    pub downloaded: u64,
    pub total: u64,
    pub speed: f64,
}

/// 从更新包 URL 推导保存的文件名。
///
/// Linux 发行产物是 .deb / .AppImage / .rpm（Windows 是 .exe），不能沿用
/// Windows 版写死的 `Lycia.Player_Setup.exe`——否则 Linux 上下载的 deb 会被
/// 存成 .exe，后续 `run_installer` 与用户感知全部错位。
fn derive_download_file_name(url: &str) -> String {
    const KNOWN_PACKAGE_EXTS: [&str; 7] = [
        ".exe", ".msi", ".deb", ".rpm", ".appimage", ".tar.gz", ".pacman",
    ];

    let clean_path = url.split(['?', '#']).next().unwrap_or(url);
    let file_name = clean_path.rsplit('/').next().unwrap_or(clean_path);
    let lower_name = file_name.to_lowercase();

    if !file_name.is_empty() && KNOWN_PACKAGE_EXTS.iter().any(|ext| lower_name.ends_with(ext)) {
        file_name.to_string()
    } else {
        "Lycia.Player_Setup.exe".to_string()
    }
}

/// 判断是否为 Windows 安装包（deb/rpm 环境下应直接拒绝执行）。
fn is_windows_installer_file(path: &std::path::Path) -> bool {
    matches!(
        path.extension().and_then(|ext| ext.to_str()),
        Some(ext) if ext.eq_ignore_ascii_case("exe") || ext.eq_ignore_ascii_case("msi")
    )
}

#[tauri::command]
pub async fn download_update_file(
    app_handle: tauri::AppHandle,
    url: String,
) -> Result<String, String> {
    use std::time::Instant;
    use tauri::{Emitter, Manager};
    use tokio::fs::File;
    use tokio::io::AsyncWriteExt;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(300))
        .user_agent("LyciaPlayer-Updater")
        .build()
        .map_err(|e| format!("创建下载请求客户端失败: {e}"))?;

    let mut download_url = url.clone();
    if download_url.contains("github.com") {
        download_url = format!("https://gh-proxy.com/{}", download_url);
    }

    let response = client
        .get(&download_url)
        .send()
        .await
        .map_err(|e| format!("发送下载请求失败: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("下载服务器返回错误状态: {}", response.status()));
    }

    let total_size = response.content_length().unwrap_or(0);
    let download_dir = app_handle
        .path()
        .download_dir()
        .map_err(|e| e.to_string())?;

    let filename = derive_download_file_name(&url);
    let dest_path = download_dir.join(filename);

    let mut file = File::create(&dest_path)
        .await
        .map_err(|e| format!("创建目标文件失败: {e}"))?;
    let mut downloaded: u64 = 0;
    let start_time = Instant::now();
    let mut last_emit = Instant::now();

    let mut response = response;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| format!("下载数据分块失败: {e}"))?
    {
        file.write_all(&chunk)
            .await
            .map_err(|e| format!("写入文件失败: {e}"))?;
        downloaded += chunk.len() as u64;

        let now = Instant::now();
        if now.duration_since(last_emit).as_millis() >= 100 || downloaded == total_size {
            let elapsed = start_time.elapsed().as_secs_f64();
            let speed = if elapsed > 0.0 {
                downloaded as f64 / elapsed
            } else {
                0.0
            };
            let progress = if total_size > 0 {
                (downloaded as f64 / total_size as f64) * 100.0
            } else {
                0.0
            };

            let payload = DownloadProgress {
                progress,
                downloaded,
                total: total_size,
                speed,
            };
            let _ = app_handle.emit("update-download-progress", payload);
            last_emit = now;
        }
    }

    file.flush()
        .await
        .map_err(|e| format!("刷新文件缓存失败: {e}"))?;

    Ok(dest_path.to_string_lossy().to_string())
}

#[tauri::command]
pub fn run_installer(path: String) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        use std::process::Command;

        Command::new("cmd")
            .args(&["/C", "start", "", &path])
            .spawn()
            .map_err(|e| format!("启动安装程序失败: {e}"))?;
        Ok(())
    }

    #[cfg(not(target_os = "windows"))]
    {
        linux_run_installer(std::path::PathBuf::from(path))
    }
}

/// Linux 安装/升级入口。
///
/// - deb / rpm：交给桌面环境的软件中心（GNOME Software、KDE Discover、
///   GNOME 桌面下的 qapt 等）通过 xdg-open/gio 打开，由它完成 pkexec 提权
///   与依赖解析；应用自身绝不直接以 root 执行 dpkg/rpm。
/// - AppImage：设置可执行位后执行自更新——用下载文件原子替换 `$APPIMAGE`
///   指向的旧文件（POSIX rename 允许替换运行中的可执行文件），再由一个
///   与本进程解耦的 shell 等待旧进程退出后拉起新版本。直接 spawn 新实例
///   会被 single-instance 插件当作重复启动拦截，因此必须先退出旧进程。
#[cfg(not(target_os = "windows"))]
fn linux_run_installer(installer_path: std::path::PathBuf) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    use std::process::{Command, Stdio};

    if is_windows_installer_file(&installer_path) {
        return Err(
            "Windows 安装包无法在 Linux 上安装，请从 Releases 页面下载 .deb 或 .AppImage".into(),
        );
    }

    if !installer_path.exists() {
        return Err(format!("安装包不存在: {}", installer_path.display()));
    }

    let extension = installer_path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_lowercase();

    if matches!(extension.as_str(), "deb" | "rpm") {
        let mut last_error = None;
        for opener in ["xdg-open", "gio"] {
            let mut command = Command::new(opener);
            if opener == "gio" {
                command.arg("open");
            }
            command.arg(&installer_path);
            match command.spawn() {
                Ok(_) => return Ok(()),
                Err(error) => last_error = Some(error),
            }
        }
        return Err(format!(
            "无法调用系统安装器（xdg-open/gio 均不可用）：{:?}。可手动执行 \
             `sudo apt install \"{}\"` 或 `sudo dpkg -i \"{}\"`",
            last_error,
            installer_path.display(),
            installer_path.display()
        ));
    }

    let is_appimage = extension == "appimage"
        || installer_path
            .file_name()
            .and_then(|name| name.to_str())
            .map(|name| name.to_lowercase().contains("appimage"))
            .unwrap_or(false);

    std::fs::set_permissions(&installer_path, std::fs::Permissions::from_mode(0o755))
        .map_err(|e| format!("设置可执行权限失败: {e}"))?;

    let launch_target = if is_appimage {
        match std::env::var_os("APPIMAGE").map(std::path::PathBuf::from) {
            Some(current_appimage) => {
                // 自更新：先把下载文件落到旧 AppImage 同目录（保证同一文件
                // 系统，rename 才是原子替换），再覆盖旧文件。
                let staged = current_appimage.with_extension("appimage.new");
                let _ = std::fs::remove_file(&staged);
                std::fs::copy(&installer_path, &staged)
                    .map_err(|e| format!("暂存新版本失败: {e}"))?;
                std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))
                    .map_err(|e| format!("设置可执行权限失败: {e}"))?;
                std::fs::rename(&staged, &current_appimage)
                    .map_err(|e| format!("替换旧版本失败: {e}"))?;
                let _ = std::fs::remove_file(&installer_path);
                current_appimage
            }
            None => installer_path,
        }
    } else {
        // 未知格式（tar.gz 等）：不擅自执行，交还用户处理。
        return Err(format!(
            "暂不支持自动安装该文件格式（.{extension}），请手动安装后重启应用"
        ));
    };

    let old_pid = std::process::id();
    Command::new("sh")
        .arg("-c")
        .arg(format!(
            "while kill -0 {old_pid} 2>/dev/null; do sleep 0.3; done; exec \"$1\""
        ))
        .arg("sh")
        .arg(&launch_target)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("启动更新任务失败: {e}"))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derives_file_name_from_update_url() {
        assert_eq!(
            derive_download_file_name(
                "https://github.com/a/b/releases/download/v1.4.0/Lycia.Player_1.4.0_amd64.deb"
            ),
            "Lycia.Player_1.4.0_amd64.deb"
        );
        assert_eq!(
            derive_download_file_name(
                "https://gh-proxy.com/https://github.com/a/b/releases/download/v2.0.0/Lycia-Player_2.0.0_amd64.AppImage?token=x#y"
            ),
            "Lycia-Player_2.0.0_amd64.AppImage"
        );
        assert_eq!(
            derive_download_file_name("https://example.com/lycia/Lycia.Player_Setup_Standard.exe"),
            "Lycia.Player_Setup_Standard.exe"
        );
        // 未知扩展名回退到 Windows 默认名，保持旧行为。
        assert_eq!(
            derive_download_file_name("https://example.com/no-extension"),
            "Lycia.Player_Setup.exe"
        );
    }

    #[test]
    fn detects_windows_installers() {
        assert!(is_windows_installer_file(std::path::Path::new(
            "downloads/Lycia.Player_Setup.exe"
        )));
        assert!(is_windows_installer_file(std::path::Path::new(
            "downloads/setup.MSI"
        )));
        assert!(!is_windows_installer_file(std::path::Path::new(
            "downloads/Lycia.Player_1.4.0_amd64.deb"
        )));
        assert!(!is_windows_installer_file(std::path::Path::new(
            "downloads/Lycia.Player.AppImage"
        )));
    }
}
