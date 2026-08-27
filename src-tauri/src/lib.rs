use std::{
    env, fs,
    fs::OpenOptions,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
    sync::Mutex,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sysinfo::{Components, Disks, Networks, System};
#[cfg(target_os = "windows")]
use windows_sys::Win32::{
    Foundation::{HWND, POINT, RECT, SYSTEMTIME},
    Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTOPRIMARY,
    },
    System::SystemInformation::GetLocalTime,
    UI::{
        HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI},
        WindowsAndMessaging::{IsWindow, MessageBoxW, IDYES, MB_ICONWARNING, MB_OK, MB_YESNO},
    },
};
use tauri::{
    tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, LogicalSize, Manager, PhysicalPosition, WebviewWindowBuilder,
};

#[cfg(target_os = "windows")]
mod taskbar;
mod taskbar_transparency;
mod proxy_config;

use taskbar_transparency::TaskbarTransparencyManager;

const CODEX_USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";
const CODEX_USAGE_PATH: &str = "/backend-api/wham/usage";
const CODEX_RESET_CREDITS_URL: &str = "https://chatgpt.com/backend-api/wham/rate-limit-reset-credits";
const CODEX_RESET_CREDITS_PATH: &str = "/backend-api/wham/rate-limit-reset-credits";
const LOG_FILE_NAME: &str = "widget.log";
const DATABASE_FILE_NAME: &str = "widget-history.sqlite";
const GPT_REFRESH_SECONDS: u64 = 60;
const HISTORY_WRITE_INTERVAL_SECONDS: i64 = 60;
const SETTINGS_MARGIN: f64 = 8.0;
const SETTINGS_MIN_WIDTH: f64 = 320.0;

fn clamp_settings_size(
    measured_width: f64,
    measured_height: f64,
    work_area_width: f64,
    work_area_height: f64,
) -> (f64, f64) {
    let available_width = (work_area_width - SETTINGS_MARGIN * 2.0).max(0.0);
    let available_height = (work_area_height - SETTINGS_MARGIN * 2.0).max(0.0);
    let width = measured_width
        .max(SETTINGS_MIN_WIDTH.min(available_width))
        .min(available_width);
    let height = measured_height.max(0.0).min(available_height);
    (width, height)
}

fn main_window_needs_rebuild(window_exists: bool, native_window_valid: bool) -> bool {
    !window_exists || !native_window_valid
}

#[derive(Copy, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum WidgetPosition {
    Left,
    Right,
}

#[derive(Copy, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum GptDisplayMode {
    Remaining,
    Used,
}

#[derive(Copy, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum TaskbarTransparencyMode {
    Off,
    Clear,
}

#[derive(Copy, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum AppLanguage {
    En,
    Zh,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(default)]
struct WidgetConfig {
    position: WidgetPosition,
    cpu_visible: bool,
    memory_visible: bool,
    gpt_visible: bool,
    temperature_visible: bool,
    disk_visible: bool,
    upload_visible: bool,
    download_visible: bool,
    gpt_refresh_seconds: u64,
    gpt_display_mode: GptDisplayMode,
    gpt_proxy_url: String,
    taskbar_transparency_mode: TaskbarTransparencyMode,
    language: AppLanguage,
}

impl Default for WidgetConfig {
    /// 创建所有指标默认显示且 GPT 每分钟刷新的配置。
    fn default() -> Self {
        Self {
            position: WidgetPosition::Left,
            cpu_visible: true,
            memory_visible: true,
            gpt_visible: true,
            temperature_visible: true,
            disk_visible: true,
            upload_visible: true,
            download_visible: true,
            gpt_refresh_seconds: GPT_REFRESH_SECONDS,
            gpt_display_mode: GptDisplayMode::Remaining,
            gpt_proxy_url: String::new(),
            taskbar_transparency_mode: TaskbarTransparencyMode::Off,
            language: AppLanguage::En,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_size_fits_small_work_area() {
        assert_eq!(clamp_settings_size(432.0, 884.0, 360.0, 720.0), (344.0, 704.0));
    }

    #[test]
    fn settings_size_keeps_template_size_when_space_allows() {
        assert_eq!(clamp_settings_size(432.0, 884.0, 1920.0, 1040.0), (432.0, 884.0));
    }

    #[test]
    fn settings_size_uses_measured_content_size_when_space_allows() {
        assert_eq!(clamp_settings_size(344.0, 704.0, 1920.0, 1040.0), (344.0, 704.0));
    }

    #[test]
    fn settings_size_fits_work_area_narrower_than_minimum_width() {
        assert_eq!(clamp_settings_size(432.0, 884.0, 300.0, 200.0), (284.0, 184.0));
    }

    #[test]
    fn settings_size_stays_non_negative_for_extremely_narrow_input() {
        assert_eq!(clamp_settings_size(1.0, 1.0, 8.0, 4.0), (0.0, 0.0));
    }

    #[test]
    fn widget_config_always_normalizes_to_sixty_seconds() {
        let mut config = WidgetConfig {
            cpu_visible: false,
            gpt_refresh_seconds: 300,
            ..WidgetConfig::default()
        };

        normalize_widget_config(&mut config);

        assert!(!config.cpu_visible);
        assert_eq!(config.gpt_refresh_seconds, 60);
    }

    #[test]
    fn missing_or_invalid_main_window_requires_rebuild() {
        assert!(!main_window_needs_rebuild(true, true));
        assert!(main_window_needs_rebuild(true, false));
        assert!(main_window_needs_rebuild(false, false));
    }
}

struct WidgetState {
    config: Mutex<WidgetConfig>,
    taskbar_transparency: TaskbarTransparencyManager,
    effective_proxy_url: String,
    log_path: PathBuf,
    db_path: PathBuf,
    system_sample_cache: Mutex<Option<CachedSystemSample>>,
    gpt_sample_cache: Mutex<Option<CachedGptSample>>,
    gpt_last_remaining_percent: Mutex<Option<u8>>,
    gpt_reset_alert_pending: Mutex<bool>,
    tray_icon: Mutex<Option<TrayIcon>>,
    system: Mutex<System>,
    components: Mutex<Components>,
    disks: Mutex<Disks>,
    networks: Mutex<Networks>,
    network_instant: Mutex<Instant>,
}

struct CachedSystemSample {
    timestamp: i64,
    cpu_percent: f64,
    memory_percent: f64,
    temperature_celsius: Option<f64>,
    disk_percent: Option<f64>,
    upload_bytes_per_second: f64,
    download_bytes_per_second: f64,
}

struct CachedGptSample {
    timestamp: i64,
    remaining_percent: f64,
    used_percent: f64,
}

#[derive(Serialize)]
struct SystemUsage {
    cpu_percent: f32,
    memory_percent: f32,
    temperature_celsius: Option<f32>,
    disk_percent: Option<f32>,
    upload_bytes_per_second: f64,
    download_bytes_per_second: f64,
}

#[derive(Serialize)]
struct CodexUsage {
    primary_remaining_percent: Option<u8>,
    weekly_remaining_percent: Option<u8>,
}

#[derive(Serialize)]
struct ResetCredit {
    status: String,
    title: String,
    description: String,
    expires_at: Option<String>,
    is_supported_by_plan: bool,
}

#[derive(Serialize)]
struct CodexResetCredits {
    available_count: u32,
    total_earned_count: u32,
    credits: Vec<ResetCredit>,
}

#[derive(Serialize)]
struct GptResetAlertState {
    pending: bool,
}

#[derive(Deserialize)]
struct HistoryQuery {
    metric: String,
    range: String,
}

#[derive(Serialize)]
struct HistoryPoint {
    timestamp: i64,
    value: f64,
    secondary_value: Option<f64>,
}

/// 将 GPT 刷新间隔固定为一分钟。
fn normalize_widget_config(config: &mut WidgetConfig) {
    config.gpt_refresh_seconds = GPT_REFRESH_SECONDS;
}

#[cfg(target_os = "windows")]
const WINDOWS_PERSONALIZE_REG_KEY: &str =
    r"HKCU\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize";

#[cfg(target_os = "windows")]
fn should_prepare_system_transparency(previous: &WidgetConfig, next: &WidgetConfig) -> bool {
    previous.taskbar_transparency_mode != TaskbarTransparencyMode::Clear
        && next.taskbar_transparency_mode == TaskbarTransparencyMode::Clear
}

#[cfg(target_os = "windows")]
fn ensure_system_transparency_enabled(log_path: &Path) -> Result<(), String> {
    match system_transparency_disabled() {
        Ok(true) => {
            append_error_log(log_path, "任务栏透明诊断 stage=system-transparency-disabled");
            if !confirm_enable_system_transparency() {
                append_error_log(log_path, "任务栏透明诊断 stage=enable-system-transparency-cancelled");
                return Err("已取消开启系统透明效果".to_string());
            }
            enable_system_transparency_and_restart_explorer(log_path)
        }
        Ok(false) => Ok(()),
        Err(error) => {
            append_error_log(
                log_path,
                &format!("任务栏透明诊断 stage=check-system-transparency error={error}"),
            );
            Ok(())
        }
    }
}

#[cfg(target_os = "windows")]
fn system_transparency_disabled() -> Result<bool, String> {
    let output = Command::new("C:\\Windows\\System32\\reg.exe")
        .args([
            "query",
            WINDOWS_PERSONALIZE_REG_KEY,
            "/v",
            "EnableTransparency",
        ])
        .output()
        .map_err(|error| format!("无法读取系统透明效果注册表：{error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(format!(
            "读取系统透明效果注册表失败 status={} stderr={stderr}",
            output.status
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines().filter(|line| line.contains("EnableTransparency")) {
        let value = line.split_whitespace().last().unwrap_or_default();
        return Ok(value == "0" || value.eq_ignore_ascii_case("0x0"));
    }
    Err("系统透明效果注册表值不存在".to_string())
}

#[cfg(target_os = "windows")]
fn confirm_enable_system_transparency() -> bool {
    let title: Vec<u16> = "任务栏透明\0".encode_utf16().collect();
    let message: Vec<u16> =
        "系统“透明效果”当前未开启。是否立即开启并重启 Explorer 外壳程序？\0"
            .encode_utf16()
            .collect();
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            message.as_ptr(),
            title.as_ptr(),
            MB_YESNO | MB_ICONWARNING,
        ) == IDYES
    }
}

#[cfg(target_os = "windows")]
fn enable_system_transparency_and_restart_explorer(log_path: &Path) -> Result<(), String> {
    let output = Command::new("C:\\Windows\\System32\\reg.exe")
        .args([
            "add",
            WINDOWS_PERSONALIZE_REG_KEY,
            "/v",
            "EnableTransparency",
            "/t",
            "REG_DWORD",
            "/d",
            "1",
            "/f",
        ])
        .output()
        .map_err(|error| format!("无法开启系统透明效果：{error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(format!(
            "开启系统透明效果失败 status={} stderr={stderr}",
            output.status
        ));
    }
    append_error_log(log_path, "任务栏透明诊断 stage=enable-system-transparency-ok");

    let output = Command::new("C:\\Windows\\System32\\taskkill.exe")
        .args(["/F", "/IM", "explorer.exe"])
        .output()
        .map_err(|error| format!("无法停止 Explorer 外壳程序：{error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(format!("停止 Explorer 外壳程序失败 status={} stderr={stderr}", output.status));
    }
    Command::new("C:\\Windows\\explorer.exe")
        .spawn()
        .map_err(|error| format!("无法启动 Explorer 外壳程序：{error}"))?;
    append_error_log(log_path, "任务栏透明诊断 stage=restart-explorer-ok");
    Ok(())
}

/// 获取用于日志前缀的本地日期时间。
#[cfg(target_os = "windows")]
fn local_log_timestamp() -> String {
    let mut local_time: SYSTEMTIME = unsafe { std::mem::zeroed() };
    unsafe { GetLocalTime(&mut local_time) };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        local_time.wYear,
        local_time.wMonth,
        local_time.wDay,
        local_time.wHour,
        local_time.wMinute,
        local_time.wSecond
    )
}

/// 获取非 Windows 环境使用的日志时间占位文本。
#[cfg(not(target_os = "windows"))]
fn local_log_timestamp() -> String {
    "0000-00-00 00:00:00".to_string()
}

/// 将错误消息追加到组件日志文件，不记录认证令牌或响应正文。
fn append_error_log(path: &Path, message: &str) {
    let timestamp = local_log_timestamp();
    let result = path
        .parent()
        .map(fs::create_dir_all)
        .transpose()
        .and_then(|_| {
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
        })
        .and_then(|mut file| writeln!(file, "[{timestamp}] ERROR {message}"));
    if let Err(error) = result {
        eprintln!("无法写入组件日志文件 {}：{error}", path.display());
    }
}

/// 使用 Windows 记事本打开组件日志文件。
///
/// # Errors
/// 当日志文件创建或记事本启动失败时返回中文错误信息。
fn open_log_file(path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("无法创建组件日志目录 {}：{error}", parent.display()))?;
    }
    if !path.exists() {
        fs::write(path, "")
            .map_err(|error| format!("无法创建组件日志文件 {}：{error}", path.display()))?;
    }
    Command::new("C:\\Windows\\System32\\notepad.exe")
        .arg(path)
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("无法使用记事本打开组件日志 {}：{error}", path.display()))
}

/// 获取当前 Unix 秒级时间戳。
fn unix_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

/// 初始化本地 SQLite 历史数据库。
///
/// # Errors
/// 当数据库打开或建表失败时返回中文错误信息。
fn init_history_database(path: &Path) -> Result<(), String> {
    let connection = Connection::open(path)
        .map_err(|error| format!("无法打开历史数据库 {}：{error}", path.display()))?;
    connection
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS system_samples (
                timestamp INTEGER NOT NULL,
                cpu_percent REAL NOT NULL,
                memory_percent REAL NOT NULL,
                temperature_celsius REAL,
                disk_percent REAL,
                upload_bytes_per_second REAL NOT NULL,
                download_bytes_per_second REAL NOT NULL
            );
            CREATE TABLE IF NOT EXISTS gpt_samples (
                timestamp INTEGER NOT NULL,
                remaining_percent REAL NOT NULL,
                used_percent REAL NOT NULL
            );
            CREATE TABLE IF NOT EXISTS widget_config (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                content TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_system_samples_timestamp ON system_samples(timestamp);
            CREATE INDEX IF NOT EXISTS idx_gpt_samples_timestamp ON gpt_samples(timestamp);",
        )
        .map_err(|error| format!("无法初始化历史数据库表：{error}"))?;
    let _ = connection.execute("ALTER TABLE system_samples ADD COLUMN temperature_celsius REAL", []);
    let _ = connection.execute("ALTER TABLE system_samples ADD COLUMN disk_percent REAL", []);
    Ok(())
}

/// 缓存系统指标采样，并在达到写入间隔后写入 SQLite。
fn cache_system_sample(state: &WidgetState, usage: &SystemUsage) {
    let now = unix_timestamp();
    let sample = CachedSystemSample {
        timestamp: now,
        cpu_percent: usage.cpu_percent as f64,
        memory_percent: usage.memory_percent as f64,
        temperature_celsius: usage.temperature_celsius.map(|value| value as f64),
        disk_percent: usage.disk_percent.map(|value| value as f64),
        upload_bytes_per_second: usage.upload_bytes_per_second,
        download_bytes_per_second: usage.download_bytes_per_second,
    };

    let mut cache = match state.system_sample_cache.lock() {
        Ok(cache) => cache,
        Err(error) => {
            eprintln!("无法锁定系统指标采样缓存：{error}");
            return;
        }
    };
    let should_write = cache
        .as_ref()
        .map(|cached| now.saturating_sub(cached.timestamp) >= HISTORY_WRITE_INTERVAL_SECONDS)
        .unwrap_or(false);
    if should_write {
        flush_system_sample(&state.db_path, &cache);
        *cache = Some(sample);
    } else if cache.is_none() {
        *cache = Some(sample);
    }
}

/// 将缓存中的系统指标采样写入 SQLite。
fn flush_system_sample(path: &Path, cache: &Option<CachedSystemSample>) {
    let Some(sample) = cache else {
        return;
    };
    let result = Connection::open(path).and_then(|connection| {
        connection.execute(
            "INSERT INTO system_samples (timestamp, cpu_percent, memory_percent, temperature_celsius, disk_percent, upload_bytes_per_second, download_bytes_per_second) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![sample.timestamp, sample.cpu_percent, sample.memory_percent, sample.temperature_celsius, sample.disk_percent, sample.upload_bytes_per_second, sample.download_bytes_per_second],
        )
    });
    if let Err(error) = result {
        eprintln!("写入系统指标历史采样失败：{error}");
    }
}

/// 缓存 GPT 用量采样，并在达到写入间隔后写入 SQLite。
fn cache_gpt_sample(state: &WidgetState, usage: &CodexUsage) {
    let Some(remaining) = usage.weekly_remaining_percent.or(usage.primary_remaining_percent) else {
        return;
    };
    let now = unix_timestamp();
    let sample = CachedGptSample {
        timestamp: now,
        remaining_percent: remaining as f64,
        used_percent: 100.0 - remaining as f64,
    };

    let mut cache = match state.gpt_sample_cache.lock() {
        Ok(cache) => cache,
        Err(error) => {
            eprintln!("无法锁定 GPT 用量采样缓存：{error}");
            return;
        }
    };
    let should_write = cache
        .as_ref()
        .map(|cached| now.saturating_sub(cached.timestamp) >= HISTORY_WRITE_INTERVAL_SECONDS)
        .unwrap_or(false);
    if should_write {
        flush_gpt_sample(&state.db_path, &cache);
        *cache = Some(sample);
    } else if cache.is_none() {
        *cache = Some(sample);
    }
}

/// 将缓存中的 GPT 用量采样写入 SQLite。
fn flush_gpt_sample(path: &Path, cache: &Option<CachedGptSample>) {
    let Some(sample) = cache else {
        return;
    };
    let result = Connection::open(path).and_then(|connection| {
        connection.execute(
            "INSERT INTO gpt_samples (timestamp, remaining_percent, used_percent) VALUES (?1, ?2, ?3)",
            params![sample.timestamp, sample.remaining_percent, sample.used_percent],
        )
    });
    if let Err(error) = result {
        eprintln!("写入 GPT 用量历史采样失败：{error}");
    }
}

/// 根据 GPT 剩余额度变化判断是否需要触发重置提醒。
fn update_gpt_reset_alert(state: &WidgetState, usage: &CodexUsage) {
    let Some(remaining) = usage.weekly_remaining_percent.or(usage.primary_remaining_percent) else {
        return;
    };
    let mut last_remaining = match state.gpt_last_remaining_percent.lock() {
        Ok(last_remaining) => last_remaining,
        Err(error) => {
            eprintln!("无法锁定 GPT 上次余量状态：{error}");
            return;
        }
    };
    let should_alert = last_remaining
        .map(|last| last < 100 && remaining == 100)
        .unwrap_or(false);
    *last_remaining = Some(remaining);
    drop(last_remaining);

    if should_alert {
        match state.gpt_reset_alert_pending.lock() {
            Ok(mut pending) => *pending = true,
            Err(error) => eprintln!("无法锁定 GPT 重置提醒状态：{error}"),
        }
    }
}

/// 根据代理配置创建 GPT 请求客户端。
///
/// # Errors
/// 当代理地址无效或客户端创建失败时返回中文错误信息。
fn build_codex_client(proxy_url: &str) -> Result<reqwest::Client, String> {
    let mut client_builder = reqwest::Client::builder().timeout(Duration::from_secs(30));
    if !proxy_url.is_empty() {
        let proxy = reqwest::Proxy::all(proxy_url)
            .map_err(|_| "GPT 代理地址无效，请检查命令行 --proxy 或运行目录 config.json 中的 proxy".to_string())?;
        client_builder = client_builder.proxy(proxy);
    }
    client_builder
        .build()
        .map_err(|error| format!("无法创建 Codex 请求客户端：{error}"))
}

/// 将组件配置序列化并写入 SQLite。
///
/// # Errors
/// 当数据库打开、JSON 序列化或配置写入失败时返回中文错误信息。
fn save_widget_config(path: &Path, config: &WidgetConfig) -> Result<(), String> {
    let mut config = config.clone();
    normalize_widget_config(&mut config);
    let content = serde_json::to_string(&config)
        .map_err(|error| format!("无法序列化组件配置：{error}"))?;
    let connection = Connection::open(path)
        .map_err(|error| format!("无法打开配置数据库 {}：{error}", path.display()))?;
    connection
        .execute(
            "INSERT INTO widget_config (id, content) VALUES (1, ?1) ON CONFLICT(id) DO UPDATE SET content = excluded.content",
            params![content],
        )
        .map(|_| ())
        .map_err(|error| format!("无法写入 SQLite 组件配置：{error}"))
}

/// 从 SQLite 加载组件配置，缺失或无效时保存并返回默认配置。
fn load_widget_config(path: &Path) -> WidgetConfig {
    let load_result = Connection::open(path).and_then(|connection| {
        connection.query_row(
            "SELECT content FROM widget_config WHERE id = 1",
            [],
            |row| row.get::<_, String>(0),
        )
    });

    match load_result {
        Ok(content) => match serde_json::from_str::<WidgetConfig>(&content) {
            Ok(mut config) => {
                let needs_save = config.gpt_refresh_seconds != GPT_REFRESH_SECONDS;
                normalize_widget_config(&mut config);
                if needs_save {
                    if let Err(error) = save_widget_config(path, &config) {
                        eprintln!("保存规范化组件配置失败：{error}");
                    }
                }
                config
            }
            Err(error) => {
                eprintln!("SQLite 组件配置无效，将恢复默认配置：{error}");
                let config = WidgetConfig::default();
                if let Err(save_error) = save_widget_config(path, &config) {
                    eprintln!("保存默认组件配置失败：{save_error}");
                }
                config
            }
        },
        Err(rusqlite::Error::QueryReturnedNoRows) => {
            let config = WidgetConfig::default();
            if let Err(save_error) = save_widget_config(path, &config) {
                eprintln!("创建默认 SQLite 组件配置失败：{save_error}");
            }
            config
        }
        Err(error) => {
            eprintln!("读取 SQLite 组件配置失败，将恢复默认配置：{error}");
            let config = WidgetConfig::default();
            if let Err(save_error) = save_widget_config(path, &config) {
                eprintln!("保存默认组件配置失败：{save_error}");
            }
            config
        }
    }
}

/// 根据 Vue 页面上报的逻辑宽度和当前位置调整任务栏组件窗口。
///
/// # Errors
/// 当配置锁定、窗口挂载或 Win32 调整窗口失败时返回中文错误信息。
#[tauri::command]
fn set_widget_width(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, WidgetState>,
    width: f64,
) -> Result<i32, String> {
    let position = state
        .config
        .lock()
        .map(|config| config.position)
        .map_err(|error| format!("无法锁定组件配置状态：{error}"))?;

    #[cfg(target_os = "windows")]
    {
        let result = taskbar::set_width(&window, width, position);
        if let Err(error) = &result {
            append_error_log(&state.log_path, &format!("任务栏组件重排失败：{error}"));
        }
        result
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = (window, width, position);
        Err("任务栏组件仅支持 Windows".to_string())
    }
}

/// 获取当前持久化的组件显示和 GPT 刷新配置。
///
/// # Errors
/// 当配置状态锁定失败时返回中文错误信息。
#[tauri::command]
fn get_widget_config(state: tauri::State<'_, WidgetState>) -> Result<WidgetConfig, String> {
    state
        .config
        .lock()
        .map(|config| config.clone())
        .map_err(|error| format!("无法锁定组件配置状态：{error}"))
}

/// 保存设置窗口提交的组件配置，并广播给任务栏组件。
///
/// # Errors
/// 当配置值无效、状态锁定、文件写入或事件发送失败时返回中文错误信息。
#[tauri::command]
fn save_widget_settings(
    app: AppHandle,
    state: tauri::State<'_, WidgetState>,
    mut config: WidgetConfig,
) -> Result<WidgetConfig, String> {
    normalize_widget_config(&mut config);
    let previous_config = state
        .config
        .lock()
        .map(|config| config.clone())
        .map_err(|error| format!("无法锁定组件配置状态：{error}"))?;
    #[cfg(target_os = "windows")]
    if should_prepare_system_transparency(&previous_config, &config) {
        ensure_system_transparency_enabled(&state.log_path)?;
    }
    {
        let mut current = state
            .config
            .lock()
            .map_err(|error| format!("无法锁定组件配置状态：{error}"))?;
        *current = config.clone();
    }
    save_widget_config(&state.db_path, &config)?;
    if previous_config.taskbar_transparency_mode != config.taskbar_transparency_mode {
        let _ = state
            .taskbar_transparency
            .set_mode(config.taskbar_transparency_mode);
    }
    app.emit("widget-config-changed", &config)
        .map_err(|error| format!("发送组件配置变更事件失败：{error}"))?;
    Ok(config)
}

/// 隐藏设置窗口。
///
/// # Errors
/// 当找不到窗口或隐藏失败时返回中文错误信息。
#[tauri::command]
fn hide_settings_window(app: AppHandle) -> Result<(), String> {
    let window = app
        .get_webview_window("settings")
        .ok_or_else(|| "找不到 settings 设置窗口".to_string())?;
    window
        .hide()
        .map_err(|error| format!("无法隐藏设置窗口：{error}"))
}

/// 获取主显示器同一时刻的物理工作区与有效 DPI。
#[cfg(target_os = "windows")]
fn primary_work_area() -> Result<(RECT, u32), String> {
    let monitor = unsafe {
        MonitorFromPoint(
            POINT { x: 0, y: 0 },
            MONITOR_DEFAULTTOPRIMARY,
        )
    };
    if monitor.is_null() {
        return Err("无法定位 Windows 主显示器".to_string());
    }

    let mut monitor_info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        rcMonitor: RECT::default(),
        rcWork: RECT::default(),
        dwFlags: 0,
    };
    if unsafe { GetMonitorInfoW(monitor, &mut monitor_info) } == 0
        || monitor_info.rcWork.right <= monitor_info.rcWork.left
        || monitor_info.rcWork.bottom <= monitor_info.rcWork.top
    {
        return Err("无法获取 Windows 主显示器工作区".to_string());
    }

    let mut dpi_x = 96_u32;
    let mut dpi_y = 96_u32;
    let dpi_result = unsafe {
        GetDpiForMonitor(
            monitor,
            MDT_EFFECTIVE_DPI,
            &mut dpi_x,
            &mut dpi_y,
        )
    };
    let dpi = if dpi_result >= 0 && dpi_x > 0 {
        dpi_x
    } else {
        96
    };
    Ok((monitor_info.rcWork, dpi))
}

/// 根据设置面板内容与主显示器工作区调整设置窗口。
///
/// # Errors
/// 当找不到窗口或调整窗口失败时返回中文错误信息。
#[tauri::command]
fn resize_settings_window(app: AppHandle, width: f64, height: f64) -> Result<(), String> {
    let window = app
        .get_webview_window("settings")
        .ok_or_else(|| "找不到 settings 设置窗口".to_string())?;

    #[cfg(target_os = "windows")]
    {
        let (work_area, dpi) = primary_work_area()?;
        let scale = f64::from(dpi) / 96.0;
        let work_area_width = f64::from(work_area.right - work_area.left) / scale;
        let work_area_height = f64::from(work_area.bottom - work_area.top) / scale;
        let (next_width, next_height) =
            clamp_settings_size(width, height, work_area_width, work_area_height);
        let runtime_min_width = SETTINGS_MIN_WIDTH
            .min((work_area_width - SETTINGS_MARGIN * 2.0).max(0.0));
        window
            .set_min_size(Some(LogicalSize::new(runtime_min_width, 0.0)))
            .map_err(|error| format!("无法调整设置窗口最小尺寸：{error}"))?;
        window
            .set_size(LogicalSize::new(next_width, next_height))
            .map_err(|error| format!("无法调整设置窗口尺寸：{error}"))?;
        position_settings_window_in_work_area(&window, &work_area)?;
    }

    #[cfg(not(target_os = "windows"))]
    window
        .set_size(LogicalSize::new(width, height))
        .map_err(|error| format!("无法调整设置窗口尺寸：{error}"))?;

    Ok(())
}

/// 获取当前系统资源、磁盘、温度和网络速率。
///
/// # Errors
/// 当任一持久采样器状态锁定失败或内存总量无效时返回中文错误信息。
#[tauri::command]
fn get_system_usage(state: tauri::State<'_, WidgetState>) -> Result<SystemUsage, String> {
    let mut system = state
        .system
        .lock()
        .map_err(|error| format!("无法锁定系统信息状态：{error}"))?;
    system.refresh_cpu_usage();
    system.refresh_memory();

    let total_memory = system.total_memory();
    if total_memory == 0 {
        return Err("无法读取系统内存总量".to_string());
    }
    let cpu_percent = system.global_cpu_usage().clamp(0.0, 100.0);
    let memory_percent =
        (system.used_memory() as f64 / total_memory as f64 * 100.0).clamp(0.0, 100.0) as f32;
    drop(system);

    let mut components = state
        .components
        .lock()
        .map_err(|error| format!("无法锁定温度采样状态：{error}"))?;
    components.refresh(true);
    let temperature_celsius = components
        .iter()
        .filter_map(|component| component.temperature())
        .filter(|temperature| temperature.is_finite())
        .max_by(|left, right| left.total_cmp(right));
    drop(components);

    let mut disks = state
        .disks
        .lock()
        .map_err(|error| format!("无法锁定磁盘采样状态：{error}"))?;
    disks.refresh(true);
    let total_disk_bytes: u64 = disks.iter().map(|disk| disk.total_space()).sum();
    let available_disk_bytes: u64 = disks.iter().map(|disk| disk.available_space()).sum();
    let disk_percent = (total_disk_bytes > 0).then(|| {
        ((total_disk_bytes.saturating_sub(available_disk_bytes)) as f64
            / total_disk_bytes as f64
            * 100.0)
            .clamp(0.0, 100.0) as f32
    });
    drop(disks);

    let mut network_instant = state
        .network_instant
        .lock()
        .map_err(|error| format!("无法锁定网络计时状态：{error}"))?;
    let elapsed_seconds = network_instant.elapsed().as_secs_f64().max(f64::EPSILON);
    let mut networks = state
        .networks
        .lock()
        .map_err(|error| format!("无法锁定网络采样状态：{error}"))?;
    networks.refresh(true);
    let download_bytes: u64 = networks.iter().map(|(_, network)| network.received()).sum();
    let upload_bytes: u64 = networks.iter().map(|(_, network)| network.transmitted()).sum();
    *network_instant = Instant::now();

    let usage = SystemUsage {
        cpu_percent,
        memory_percent,
        temperature_celsius,
        disk_percent,
        upload_bytes_per_second: upload_bytes as f64 / elapsed_seconds,
        download_bytes_per_second: download_bytes as f64 / elapsed_seconds,
    };
    cache_system_sample(state.inner(), &usage);
    Ok(usage)
}

/// 打开 Windows 任务管理器。
///
/// # Errors
/// 当系统无法启动 taskmgr.exe 时返回中文错误信息。
#[tauri::command]
fn open_task_manager() -> Result<(), String> {
    Command::new("C:\\Windows\\System32\\taskmgr.exe")
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("无法打开 Windows 任务管理器：{error}"))
}

/// 获取 Codex CLI 认证文件路径。
///
/// # Errors
/// 当 CODEX_HOME 和 USERPROFILE 都不可用时返回中文错误信息。
fn codex_auth_path() -> Result<PathBuf, String> {
    if let Ok(codex_home) = env::var("CODEX_HOME") {
        let codex_home = codex_home.trim();
        if !codex_home.is_empty() {
            return Ok(PathBuf::from(codex_home).join("auth.json"));
        }
    }

    let user_profile = env::var("USERPROFILE")
        .map_err(|error| format!("无法读取 USERPROFILE 环境变量：{error}"))?;
    Ok(PathBuf::from(user_profile).join(".codex").join("auth.json"))
}

/// 从 Codex CLI 认证文件读取访问令牌。
///
/// # Errors
/// 当文件读取、JSON 解析或令牌字段检查失败时返回中文错误信息。
fn load_codex_access_token() -> Result<String, String> {
    let auth_path = codex_auth_path()?;
    let auth_content = fs::read_to_string(&auth_path)
        .map_err(|error| format!("无法读取 Codex CLI 认证文件 {}：{error}", auth_path.display()))?;
    let auth_payload: Value = serde_json::from_str(&auth_content)
        .map_err(|error| format!("Codex CLI 认证文件不是有效 JSON：{error}"))?;
    let raw_token = auth_payload
        .pointer("/tokens/access_token")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .ok_or_else(|| "Codex CLI 认证文件中没有 access_token，请运行 codex login".to_string())?;

    let normalized_token = raw_token
        .get(..7)
        .filter(|prefix| prefix.eq_ignore_ascii_case("bearer "))
        .and_then(|_| raw_token.get(7..))
        .unwrap_or(raw_token)
        .trim();

    if normalized_token.is_empty() {
        return Err("Codex CLI access_token 为空，请运行 codex login".to_string());
    }

    Ok(normalized_token.to_string())
}

/// 从 JSON 数值或数字字符串中读取浮点数。
fn json_number(value: Option<&Value>) -> Option<f64> {
    value.and_then(|value| {
        value
            .as_f64()
            .or_else(|| value.as_str().and_then(|text| text.parse::<f64>().ok()))
    })
}

/// 将已用比例转换为剩余百分比。
fn remaining_percent(used_percent: Option<f64>) -> Option<u8> {
    let used_percent = used_percent?;
    if !used_percent.is_finite() {
        return None;
    }
    let normalized_used = if used_percent <= 1.0 {
        used_percent * 100.0
    } else {
        used_percent
    };
    Some((100.0 - normalized_used.clamp(0.0, 100.0)).round() as u8)
}

/// 选择主 Codex 限额；优先使用 rate_limit，缺失时查找 Codex 附加限额。
fn select_codex_rate_limit(payload: &Value) -> Option<&Value> {
    if let Some(rate_limit) = payload.get("rate_limit").filter(|value| value.is_object()) {
        return Some(rate_limit);
    }

    let additional_limits = payload.get("additional_rate_limits")?;
    match additional_limits {
        Value::Array(items) => items.iter().find_map(|item| {
            let name = item
                .get("limit_name")
                .or_else(|| item.get("metered_feature"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            name.to_ascii_lowercase()
                .contains("codex")
                .then(|| item.get("rate_limit").unwrap_or(item))
        }),
        Value::Object(items) => items.iter().find_map(|(name, item)| {
            let item_name = item
                .get("limit_name")
                .or_else(|| item.get("metered_feature"))
                .and_then(Value::as_str)
                .unwrap_or(name);
            item_name
                .to_ascii_lowercase()
                .contains("codex")
                .then(|| item.get("rate_limit").unwrap_or(item))
        }),
        _ => None,
    }
}

/// 请求 ChatGPT Codex 用量并返回 5 小时与每周剩余额度。
///
/// # Errors
/// 当认证文件、HTTP 请求、状态码或响应解析失败时返回不含令牌的中文错误信息。
#[tauri::command]
async fn get_codex_usage(state: tauri::State<'_, WidgetState>) -> Result<CodexUsage, String> {
    let proxy_url = state.effective_proxy_url.as_str();
    let result = async {
        let access_token = load_codex_access_token()?;
        let client = build_codex_client(proxy_url)?;

        let response = client
            .get(CODEX_USAGE_URL)
            .header("Accept", "*/*")
            .bearer_auth(access_token)
            .header("Cache-Control", "no-cache")
            .header("Pragma", "no-cache")
            .header("Referer", "https://chatgpt.com/codex/cloud/settings/analytics")
            .header("oai-language", "en-US")
            .header("x-openai-target-path", CODEX_USAGE_PATH)
            .header("x-openai-target-route", CODEX_USAGE_PATH)
            .send()
            .await
            .map_err(|error| {
                if proxy_url.is_empty() {
                    format!("无法请求 Codex 用量：{error}")
                } else {
                    "无法通过配置的 GPT 代理请求 Codex 用量，请检查代理地址和代理服务状态".to_string()
                }
            })?;

        let status = response.status();
        if status.as_u16() == 401 || status.as_u16() == 403 {
            return Err("Codex CLI 登录令牌已失效，请运行 codex login".to_string());
        }
        if !status.is_success() {
            return Err(format!("Codex 用量接口请求失败，HTTP 状态码：{}", status.as_u16()));
        }

        let payload: Value = response
            .json()
            .await
            .map_err(|error| format!("Codex 用量接口返回了无效 JSON：{error}"))?;
        let rate_limit = select_codex_rate_limit(&payload)
            .ok_or_else(|| "Codex 用量响应中没有可识别的限额信息".to_string())?;
        let primary_remaining_percent = remaining_percent(json_number(
            rate_limit.pointer("/primary_window/used_percent"),
        ));
        let weekly_remaining_percent = remaining_percent(json_number(
            rate_limit.pointer("/secondary_window/used_percent"),
        ));

        if primary_remaining_percent.is_none() && weekly_remaining_percent.is_none() {
            return Err("Codex 用量响应中没有可识别的百分比".to_string());
        }

        Ok(CodexUsage {
            primary_remaining_percent,
            weekly_remaining_percent,
        })
    }
    .await;

    if let Err(error) = &result {
        append_error_log(&state.log_path, &format!("GPT 用量读取失败：{error}"));
    }
    if let Ok(usage) = &result {
        update_gpt_reset_alert(state.inner(), usage);
        cache_gpt_sample(state.inner(), usage);
    }
    result
}

/// 请求 GPT/Codex 重置额度信息。
///
/// # Errors
/// 当认证、代理、HTTP 状态码或响应解析失败时返回中文错误信息。
#[tauri::command]
async fn get_codex_reset_credits(
    state: tauri::State<'_, WidgetState>,
) -> Result<CodexResetCredits, String> {
    let proxy_url = state.effective_proxy_url.as_str();
    let result = async {
        let access_token = load_codex_access_token()?;
        let client = build_codex_client(proxy_url)?;
        let response = client
            .get(CODEX_RESET_CREDITS_URL)
            .header("Accept", "*/*")
            .bearer_auth(access_token)
            .header("Cache-Control", "no-cache")
            .header("Pragma", "no-cache")
            .header("Referer", "https://chatgpt.com/codex")
            .header("oai-language", "en-US")
            .header("x-openai-target-path", CODEX_RESET_CREDITS_PATH)
            .header("x-openai-target-route", CODEX_RESET_CREDITS_PATH)
            .send()
            .await
            .map_err(|error| {
                if proxy_url.is_empty() {
                    format!("无法请求 GPT 重置额度：{error}")
                } else {
                    "无法通过配置的 GPT 代理请求重置额度，请检查代理地址和代理服务状态".to_string()
                }
            })?;
        let status = response.status();
        if status.as_u16() == 401 || status.as_u16() == 403 {
            return Err("Codex CLI 登录令牌已失效，请运行 codex login".to_string());
        }
        if !status.is_success() {
            return Err(format!("GPT 重置额度接口请求失败，HTTP 状态码：{}", status.as_u16()));
        }
        let payload: Value = response
            .json()
            .await
            .map_err(|error| format!("GPT 重置额度接口返回了无效 JSON：{error}"))?;
        let available_count = payload
            .get("available_count")
            .and_then(Value::as_u64)
            .unwrap_or(0) as u32;
        let total_earned_count = payload
            .get("total_earned_count")
            .and_then(Value::as_u64)
            .unwrap_or(0) as u32;
        let credits = payload
            .get("credits")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .map(|item| ResetCredit {
                        status: item
                            .get("status")
                            .and_then(Value::as_str)
                            .unwrap_or("unknown")
                            .to_string(),
                        title: item
                            .get("title")
                            .and_then(Value::as_str)
                            .unwrap_or("Reset credit")
                            .to_string(),
                        description: item
                            .get("description")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                        expires_at: item
                            .get("expires_at")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                        is_supported_by_plan: item
                            .get("is_supported_by_plan")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        Ok(CodexResetCredits {
            available_count,
            total_earned_count,
            credits,
        })
    }
    .await;
    if let Err(error) = &result {
        append_error_log(&state.log_path, &format!("GPT 重置额度读取失败：{error}"));
    }
    result
}

/// 查询本地历史曲线数据。
///
/// # Errors
/// 当查询参数无效、数据库打开或 SQL 查询失败时返回中文错误信息。
#[tauri::command]
fn get_history_points(
    state: tauri::State<'_, WidgetState>,
    query: HistoryQuery,
) -> Result<Vec<HistoryPoint>, String> {
    let seconds = match query.range.as_str() {
        "minute" => 1800,
        "hour" => 43200,
        "day" => 86400,
        _ => return Err("历史范围仅支持 minute、hour 或 day".to_string()),
    };
    let since = unix_timestamp().saturating_sub(seconds);
    let connection = Connection::open(&state.db_path)
        .map_err(|error| format!("无法打开历史数据库 {}：{error}", state.db_path.display()))?;
    match query.metric.as_str() {
        "cpu" => query_single_history(&connection, "cpu_percent", "system_samples", since),
        "memory" => query_single_history(&connection, "memory_percent", "system_samples", since),
        "temperature" => query_optional_history(&connection, "temperature_celsius", "system_samples", since),
        "disk" => query_optional_history(&connection, "disk_percent", "system_samples", since),
        "network" => query_network_history(&connection, since),
        "gpt" => query_gpt_history(&connection, since),
        _ => Err("历史指标仅支持 cpu、memory、temperature、disk、network 或 gpt".to_string()),
    }
}

/// 查询单值历史曲线。
///
/// # Errors
/// 当 SQL 查询失败时返回中文错误信息。
fn query_single_history(
    connection: &Connection,
    column: &str,
    table: &str,
    since: i64,
) -> Result<Vec<HistoryPoint>, String> {
    let sql = format!("SELECT timestamp, COALESCE({column}, 0) FROM {table} WHERE timestamp >= ?1 ORDER BY timestamp ASC");
    let mut statement = connection
        .prepare(&sql)
        .map_err(|error| format!("无法准备历史查询：{error}"))?;
    let rows = statement
        .query_map(params![since], |row| {
            Ok(HistoryPoint {
                timestamp: row.get(0)?,
                value: row.get(1)?,
                secondary_value: None,
            })
        })
        .map_err(|error| format!("无法查询历史数据：{error}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("无法读取历史数据行：{error}"))
}

/// 查询可为空的单值历史曲线。
///
/// # Errors
/// 当 SQL 查询失败时返回中文错误信息。
fn query_optional_history(
    connection: &Connection,
    column: &str,
    table: &str,
    since: i64,
) -> Result<Vec<HistoryPoint>, String> {
    let sql = format!("SELECT timestamp, {column} FROM {table} WHERE timestamp >= ?1 AND {column} IS NOT NULL ORDER BY timestamp ASC");
    let mut statement = connection
        .prepare(&sql)
        .map_err(|error| format!("无法准备可选历史查询：{error}"))?;
    let rows = statement
        .query_map(params![since], |row| {
            Ok(HistoryPoint {
                timestamp: row.get(0)?,
                value: row.get(1)?,
                secondary_value: None,
            })
        })
        .map_err(|error| format!("无法查询可选历史数据：{error}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("无法读取可选历史数据行：{error}"))
}

/// 查询网络上下行双线历史曲线。
///
/// # Errors
/// 当 SQL 查询失败时返回中文错误信息。
fn query_network_history(connection: &Connection, since: i64) -> Result<Vec<HistoryPoint>, String> {
    let mut statement = connection
        .prepare("SELECT timestamp, upload_bytes_per_second, download_bytes_per_second FROM system_samples WHERE timestamp >= ?1 ORDER BY timestamp ASC")
        .map_err(|error| format!("无法准备网络历史查询：{error}"))?;
    let rows = statement
        .query_map(params![since], |row| {
            Ok(HistoryPoint {
                timestamp: row.get(0)?,
                value: row.get(1)?,
                secondary_value: Some(row.get(2)?),
            })
        })
        .map_err(|error| format!("无法查询网络历史数据：{error}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("无法读取网络历史数据行：{error}"))
}

/// 查询 GPT 已用和余量双值历史曲线。
///
/// # Errors
/// 当 SQL 查询失败时返回中文错误信息。
fn query_gpt_history(connection: &Connection, since: i64) -> Result<Vec<HistoryPoint>, String> {
    let mut statement = connection
        .prepare("SELECT timestamp, used_percent, remaining_percent FROM gpt_samples WHERE timestamp >= ?1 ORDER BY timestamp ASC")
        .map_err(|error| format!("无法准备 GPT 历史查询：{error}"))?;
    let rows = statement
        .query_map(params![since], |row| {
            Ok(HistoryPoint {
                timestamp: row.get(0)?,
                value: row.get(1)?,
                secondary_value: Some(row.get(2)?),
            })
        })
        .map_err(|error| format!("无法查询 GPT 历史数据：{error}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("无法读取 GPT 历史数据行：{error}"))
}

/// 将设置窗口移动到给定物理工作区的右下角。
#[cfg(target_os = "windows")]
fn position_settings_window_in_work_area(
    window: &tauri::WebviewWindow,
    work_area: &RECT,
) -> Result<(), String> {
    let window_size = window
        .outer_size()
        .map_err(|error| format!("无法读取设置窗口尺寸：{error}"))?;
    let margin = SETTINGS_MARGIN as i32;
    let x = work_area
        .right
        .saturating_sub(window_size.width as i32)
        .saturating_sub(margin);
    let y = work_area
        .bottom
        .saturating_sub(window_size.height as i32)
        .saturating_sub(margin);
    window
        .set_position(PhysicalPosition::new(x, y))
        .map_err(|error| format!("无法移动设置窗口到右下角：{error}"))
}

/// 将设置窗口移动到 Windows 主显示器工作区右下角。
#[cfg(target_os = "windows")]
fn position_settings_window(window: &tauri::WebviewWindow) {
    let result = primary_work_area()
        .and_then(|(work_area, _)| position_settings_window_in_work_area(window, &work_area));
    if let Err(error) = result {
        eprintln!("{error}，设置窗口将使用默认位置");
    }
}

/// 将非 Windows 设置窗口保留在默认位置。
#[cfg(not(target_os = "windows"))]
fn position_settings_window(_window: &tauri::WebviewWindow) {}

/// 显示设置窗口并让它获得焦点。
fn show_settings_window(app: &AppHandle) {
    let Some(window) = app.get_webview_window("settings") else {
        eprintln!("找不到 settings 设置窗口");
        return;
    };
    position_settings_window(&window);
    if let Err(error) = window.show() {
        eprintln!("无法显示设置窗口：{error}");
        return;
    }
    if let Err(error) = window.emit("settings-resize-requested", ()) {
        eprintln!("无法请求设置窗口重新测量：{error}");
    }
    if let Err(error) = window.set_focus() {
        eprintln!("无法聚焦设置窗口：{error}");
    }
}

/// 查询 GPT 重置提醒是否待处理。
///
/// # Errors
/// 当提醒状态锁定失败时返回中文错误信息。
#[tauri::command]
fn get_gpt_reset_alert_state(state: tauri::State<'_, WidgetState>) -> Result<GptResetAlertState, String> {
    state
        .gpt_reset_alert_pending
        .lock()
        .map(|pending| GptResetAlertState { pending: *pending })
        .map_err(|error| format!("无法锁定 GPT 重置提醒状态：{error}"))
}

/// 切换托盘图标显示，用于前端定时驱动闪烁效果。
///
/// # Errors
/// 当托盘图标状态锁定或图标设置失败时返回中文错误信息。
#[tauri::command]
fn set_tray_flash_visible(app: AppHandle, state: tauri::State<'_, WidgetState>, visible: bool) -> Result<(), String> {
    let tray_icon = state
        .tray_icon
        .lock()
        .map_err(|error| format!("无法锁定托盘图标状态：{error}"))?;
    let Some(tray_icon) = tray_icon.as_ref() else {
        return Ok(());
    };
    if visible {
        let icon = app
            .default_window_icon()
            .cloned()
            .ok_or_else(|| "无法获取默认托盘图标".to_string())?;
        tray_icon
            .set_icon(Some(icon))
            .map_err(|error| format!("无法恢复托盘图标：{error}"))?;
    } else {
        tray_icon
            .set_icon(None)
            .map_err(|error| format!("无法隐藏托盘图标形成闪烁：{error}"))?;
    }
    Ok(())
}

/// 确认 GPT 重置提醒，停止重复提醒并弹出系统提示。
///
/// # Errors
/// 当提醒状态锁定失败时返回中文错误信息。
#[tauri::command]
fn acknowledge_gpt_reset_alert(state: tauri::State<'_, WidgetState>) -> Result<(), String> {
    acknowledge_gpt_reset_alert_inner(state.inner())
}

/// 确认 GPT 重置提醒的内部实现，供命令和托盘事件共用。
///
/// # Errors
/// 当提醒状态锁定失败时返回中文错误信息。
fn acknowledge_gpt_reset_alert_inner(state: &WidgetState) -> Result<(), String> {
    let mut pending = state
        .gpt_reset_alert_pending
        .lock()
        .map_err(|error| format!("无法锁定 GPT 重置提醒状态：{error}"))?;
    if *pending {
        *pending = false;
        drop(pending);
        show_gpt_reset_message();
    }
    Ok(())
}

/// 弹出 GPT 用量重置提示。
///
/// # Returns
/// 无返回值。
#[cfg(target_os = "windows")]
fn show_gpt_reset_message() {
    let title: Vec<u16> = "GPT 提醒\0".encode_utf16().collect();
    let message: Vec<u16> = "GPT 用量已重置\0".encode_utf16().collect();
    unsafe {
        MessageBoxW(std::ptr::null_mut(), message.as_ptr(), title.as_ptr(), MB_OK);
    }
}

/// 非 Windows 环境不弹出系统提示。
///
/// # Returns
/// 无返回值。
#[cfg(not(target_os = "windows"))]
fn show_gpt_reset_message() {}

/// 打开组件日志文件。
///
/// # Errors
/// 当日志文件创建或记事本启动失败时返回中文错误信息。
#[tauri::command]
fn open_widget_log(state: tauri::State<'_, WidgetState>) -> Result<(), String> {
    if let Err(error) = open_log_file(&state.log_path) {
        append_error_log(&state.log_path, &error);
        return Err(error);
    }
    Ok(())
}

/// 退出整个组件应用。
#[tauri::command]
fn quit_application(app: AppHandle, state: tauri::State<'_, WidgetState>) {
    let _ = state.taskbar_transparency.restore();
    app.exit(0);
}

/// 创建右键直接打开设置窗口的系统托盘图标。
///
/// # Errors
/// 当图标或托盘创建失败时返回中文错误信息。
fn setup_tray(app: &AppHandle, _config: &WidgetConfig) -> Result<TrayIcon, String> {
    let icon = app
        .default_window_icon()
        .cloned()
        .ok_or_else(|| "无法获取默认窗口图标".to_string())?;

    TrayIconBuilder::new()
        .icon(icon)
        .show_menu_on_left_click(false)
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click { button, button_state, .. } = event {
                if button_state != MouseButtonState::Up {
                    return;
                }
                if button != MouseButton::Left && button != MouseButton::Right {
                    return;
                }
                let app_handle = tray.app_handle();
                let has_alert = app_handle
                    .state::<WidgetState>()
                    .gpt_reset_alert_pending
                    .lock()
                    .map(|pending| *pending)
                    .unwrap_or(false);
                if has_alert {
                    let state = app_handle.state::<WidgetState>();
                    if let Err(error) = acknowledge_gpt_reset_alert_inner(state.inner()) {
                        eprintln!("确认 GPT 重置提醒失败：{error}");
                    }
                }
                show_settings_window(app_handle);
            }
        })
        .build(app)
        .map_err(|error| format!("无法创建系统托盘图标：{error}"))
}

/// 获取数据库和日志使用的运行目录。
/// 开发模式使用 tbwidget 工程根目录，打包模式使用可执行文件所在目录。
///
/// # Errors
/// 当打包模式无法读取当前可执行文件路径或其父目录时返回中文错误信息。
fn runtime_data_dir() -> Result<PathBuf, String> {
    if cfg!(debug_assertions) {
        return PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| "无法确定 tbwidget 工程根目录".to_string());
    }

    let executable_path = env::current_exe()
        .map_err(|error| format!("无法读取当前可执行文件路径：{error}"))?;
    executable_path
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| format!("当前可执行文件没有父目录：{}", executable_path.display()))
}

/// 初始化 SQLite 配置、日志、持久采样器和系统托盘。
///
/// # Errors
/// 当运行目录、日志文件、数据库或托盘创建失败时返回错误。
fn setup_app(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let data_dir = runtime_data_dir().map_err(std::io::Error::other)?;
    let log_path = data_dir.join(LOG_FILE_NAME);
    let db_path = data_dir.join(DATABASE_FILE_NAME);
    let cli_proxy = proxy_config::cli_proxy(env::args_os()).map_err(std::io::Error::other)?;
    fs::write(&log_path, "").map_err(|error| {
        std::io::Error::other(format!(
            "无法在每次启动时重建组件日志文件 {}：{error}",
            log_path.display()
        ))
    })?;
    let file_proxy = match proxy_config::load_proxy_file(&data_dir.join("config.json")) {
        Ok(proxy) => proxy,
        Err(error) => {
            append_error_log(&log_path, &error);
            None
        }
    };
    let effective_proxy_url = proxy_config::resolve_proxy(cli_proxy, file_proxy).unwrap_or_default();
    init_history_database(&db_path).map_err(std::io::Error::other)?;
    let config = load_widget_config(&db_path);
    let taskbar_transparency = TaskbarTransparencyManager::new(
        log_path.clone(),
        config.taskbar_transparency_mode,
    )
    .map_err(std::io::Error::other)?;
    app.manage(WidgetState {
        config: Mutex::new(config.clone()),
        taskbar_transparency,
        effective_proxy_url,
        log_path,
        db_path,
        system_sample_cache: Mutex::new(None),
        gpt_sample_cache: Mutex::new(None),
        gpt_last_remaining_percent: Mutex::new(None),
        gpt_reset_alert_pending: Mutex::new(false),
        tray_icon: Mutex::new(None),
        system: Mutex::new(System::new_all()),
        components: Mutex::new(Components::new_with_refreshed_list()),
        disks: Mutex::new(Disks::new_with_refreshed_list()),
        networks: Mutex::new(Networks::new_with_refreshed_list()),
        network_instant: Mutex::new(Instant::now()),
    });
    app.state::<WidgetState>()
        .taskbar_transparency
        .attach_app_handle(app.handle().clone())
        .map_err(std::io::Error::other)?;
    let tray_icon = setup_tray(app.handle(), &config).map_err(std::io::Error::other)?;
    match app.state::<WidgetState>().tray_icon.lock() {
        Ok(mut state_tray_icon) => *state_tray_icon = Some(tray_icon),
        Err(error) => eprintln!("无法保存托盘图标状态：{error}"),
    }
    Ok(())
}

#[cfg(target_os = "windows")]
pub(crate) enum MainWindowRecovery {
    Ready {
        window: tauri::WebviewWindow,
        rebuilt: bool,
    },
    Stale,
}

/// 在 Explorer 重建任务栏后，于 Tauri 主线程确认、销毁或重建主组件窗口。
#[cfg(target_os = "windows")]
pub(crate) fn recover_main_taskbar_window(
    app: &AppHandle,
    should_cancel: impl Fn() -> bool,
) -> Result<MainWindowRecovery, String> {
    if should_cancel() {
        return Err("main 窗口恢复已取消".to_string());
    }
    let existing = app.get_webview_window("main");
    let native_valid = existing
        .as_ref()
        .and_then(|window| window.hwnd().ok())
        .map(|native| {
            let hwnd = native.0 as HWND;
            !hwnd.is_null() && unsafe { IsWindow(hwnd) != 0 }
        })
        .unwrap_or(false);

    if should_cancel() {
        return Err("main 窗口恢复已取消".to_string());
    }

    if main_window_needs_rebuild(existing.is_some(), native_valid) {
        if let Some(stale_window) = existing {
            if should_cancel() {
                return Err("main 窗口恢复已取消".to_string());
            }
            let _ = stale_window.destroy();
            return Ok(MainWindowRecovery::Stale);
        }

        if should_cancel() {
            return Err("main 窗口恢复已取消".to_string());
        }
        let config = app
            .config()
            .app
            .windows
            .iter()
            .find(|config| config.label == "main")
            .ok_or_else(|| "tauri.conf.json 中缺少 main 窗口配置".to_string())?;
        if should_cancel() {
            return Err("main 窗口恢复已取消".to_string());
        }
        let window = WebviewWindowBuilder::from_config(app, config)
            .map_err(|error| format!("无法读取 main 窗口配置：{error}"))?
            .build()
            .map_err(|error| format!("无法重建 main 窗口：{error}"))?;
        Ok(MainWindowRecovery::Ready {
            window,
            rebuilt: true,
        })
    } else {
        let window = existing.ok_or_else(|| "main 窗口在恢复检查期间消失".to_string())?;
        Ok(MainWindowRecovery::Ready {
            window,
            rebuilt: false,
        })
    }
}

/// 在 Tauri 就绪后将主窗口挂载到任务栏。
fn handle_run_event(app_handle: &AppHandle, event: tauri::RunEvent) {
    if let tauri::RunEvent::Ready = event {
        let Some(window) = app_handle.get_webview_window("main") else {
            eprintln!("找不到 main 窗口");
            return;
        };

        if let Err(error) = window.set_ignore_cursor_events(true) {
            eprintln!("无法关闭任务栏组件的鼠标交互：{error}");
        }

        #[cfg(target_os = "windows")]
        {
            let position = match app_handle.state::<WidgetState>().config.lock() {
                Ok(config) => config.position,
                Err(error) => {
                    eprintln!("读取组件位置配置失败，将默认靠左挂载：{error}");
                    WidgetPosition::Left
                }
            };
            if let Err(error) = taskbar::reflow(&window, position) {
                append_error_log(
                    &app_handle.state::<WidgetState>().log_path,
                    &format!("任务栏组件启动重排失败：{error}"),
                );
                eprintln!("任务栏挂载失败：{error}");
                if let Err(show_error) = window.show() {
                    eprintln!("任务栏挂载失败后也无法显示 main 窗口：{show_error}");
                }
            }
        }

        #[cfg(not(target_os = "windows"))]
        {
            if let Err(show_error) = window.show() {
                eprintln!("无法显示 main 窗口：{show_error}");
            }
        }
    }
}

/// 创建 Tauri 应用，注册系统监控命令、配置状态和托盘入口。
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(setup_app)
        .invoke_handler(tauri::generate_handler![
            set_widget_width,
            get_widget_config,
            save_widget_settings,
            hide_settings_window,
            resize_settings_window,
            get_gpt_reset_alert_state,
            set_tray_flash_visible,
            acknowledge_gpt_reset_alert,
            open_widget_log,
            quit_application,
            get_system_usage,
            open_task_manager,
            get_codex_usage,
            get_codex_reset_credits,
            get_history_points
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(handle_run_event);
}
