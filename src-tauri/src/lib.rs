use std::{
    collections::BTreeMap,
    env, fs,
    fs::OpenOptions,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
#[cfg(target_os = "windows")]
use windows_sys::Win32::{
    Foundation::{POINT, RECT, SYSTEMTIME},
    Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTOPRIMARY,
    },
    System::SystemInformation::GetLocalTime,
    UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI},
};
use tauri::{
    menu::{MenuBuilder, MenuItemBuilder, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, LogicalSize, Manager, PhysicalPosition,
};

#[cfg(target_os = "windows")]
mod taskbar;
mod plugins;
mod system_monitor;


const LOG_FILE_NAME: &str = "widget.log";
const DATABASE_FILE_NAME: &str = "widget-history.sqlite";
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


#[derive(Copy, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum WidgetPosition {
    Left,
    Right,
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
    language: AppLanguage,
}

impl Default for WidgetConfig {
    fn default() -> Self {
        Self {
            position: WidgetPosition::Left,
            language: AppLanguage::Zh,
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

}

struct WidgetState {
    config: Mutex<WidgetConfig>,
    log_path: PathBuf,
    db_path: PathBuf,
    plugins_dir: PathBuf,
    plugin_config_path: PathBuf,
    plugin_popup: Mutex<Option<PluginPopupAnchor>>,
    plugin_popup_sizes: Mutex<BTreeMap<String, (f64, f64)>>,
    plugin_settings: Mutex<Option<String>>,
    tray_icon: Mutex<Option<TrayIcon>>,
    taskbar_test_background: Mutex<bool>,
    system_monitor: Mutex<system_monitor::SystemMonitorSampler>,
}

#[derive(Clone)]
struct PluginPopupAnchor {
    plugin_id: String,
    anchor_screen_x: i32,
    taskbar_top: i32,
}


/// 获取用于日志前缀的本地日期时间。
#[cfg(target_os = "windows")]
fn local_log_timestamp() -> String {
    let mut local_time: SYSTEMTIME = unsafe { std::mem::zeroed() };
    unsafe { GetLocalTime(&mut local_time) };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03}",
        local_time.wYear,
        local_time.wMonth,
        local_time.wDay,
        local_time.wHour,
        local_time.wMinute,
        local_time.wSecond,
        local_time.wMilliseconds
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

/// 将普通运行信息追加到组件日志文件。
fn append_info_log(path: &Path, message: &str) {
    let timestamp = local_log_timestamp();
    let result = path
        .parent()
        .map(fs::create_dir_all)
        .transpose()
        .and_then(|_| OpenOptions::new().create(true).append(true).open(path))
        .and_then(|mut file| writeln!(file, "[{timestamp}] INFO {message}"));
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
            "CREATE TABLE IF NOT EXISTS widget_config (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                content TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS plugin_storage (
                plugin_id TEXT NOT NULL,
                storage_key TEXT NOT NULL,
                json_value TEXT NOT NULL,
                updated_at INTEGER NOT NULL,
                PRIMARY KEY (plugin_id, storage_key)
            );",
        )
        .map_err(|error| format!("无法初始化数据库表：{error}"))?;
    Ok(())
}

/// 将组件配置序列化并写入 SQLite。
///
/// # Errors
/// 当数据库打开、JSON 序列化或配置写入失败时返回中文错误信息。
fn save_widget_config(path: &Path, config: &WidgetConfig) -> Result<(), String> {
    let content = serde_json::to_string(config)
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
            Ok(config) => config,
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TaskbarInteractiveRegion {
    id: String,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

/// 上报任务栏中的可交互矩形；原生低级鼠标 Hook 仅在这些区域内接管按键。
#[tauri::command]
fn set_taskbar_interactive_regions(
    window: tauri::WebviewWindow,
    regions: Vec<TaskbarInteractiveRegion>,
    scale_factor: f64,
) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        let native_regions = regions
            .into_iter()
            .map(|region| taskbar::InteractiveRegion {
                id: region.id,
                x: region.x,
                y: region.y,
                width: region.width,
                height: region.height,
            })
            .collect::<Vec<_>>();
        return taskbar::set_interactive_regions(&window, &native_regions, scale_factor);
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = (window, regions, scale_factor);
        Ok(())
    }
}

/// 获取 Windows 任务栏右侧通知/托盘区域宽度，供靠右布局避让。
#[tauri::command]
fn get_taskbar_right_reserved_width() -> Result<f64, String> {
    #[cfg(target_os = "windows")]
    {
        return taskbar::taskbar_right_reserved_logical_width();
    }

    #[cfg(not(target_os = "windows"))]
    {
        Ok(0.0)
    }
}

/// 获取当前持久化的底座配置。
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
    config: WidgetConfig,
) -> Result<WidgetConfig, String> {
    {
        let mut current = state
            .config
            .lock()
            .map_err(|error| format!("无法锁定组件配置状态：{error}"))?;
        *current = config.clone();
    }
    save_widget_config(&state.db_path, &config)?;
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
#[cfg(target_os = "windows")]
fn position_settings_window_in_work_area(
    window: &tauri::WebviewWindow,
    work_area: &RECT,
) -> Result<(), String> {
    let window_size = window
        .outer_size()
        .map_err(|error| format!("无法读取设置窗口尺寸：{error}"))?;
    let work_width = work_area.right.saturating_sub(work_area.left);
    let work_height = work_area.bottom.saturating_sub(work_area.top);
    let x = work_area.left + (work_width - window_size.width as i32).max(0) / 2;
    let y = work_area.top + (work_height - window_size.height as i32).max(0) / 2;
    window
        .set_position(PhysicalPosition::new(x, y))
        .map_err(|error| format!("无法将设置窗口移动到屏幕中央：{error}"))
}

/// 将设置窗口移动到 Windows 主显示器工作区中央。
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
    if let Err(error) = window.set_always_on_top(true) {
        eprintln!("无法将设置窗口置顶：{error}");
    }
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

/// 打开组件日志文件。
///
/// # Errors
/// 当日志文件创建或记事本启动失败时返回中文错误信息。
#[tauri::command]
fn get_widget_log_path(state: tauri::State<'_, WidgetState>) -> String {
    state.log_path.to_string_lossy().into_owned()
}

#[tauri::command]
fn open_widget_log(state: tauri::State<'_, WidgetState>) -> Result<(), String> {
    if let Err(error) = open_log_file(&state.log_path) {
        append_error_log(&state.log_path, &error);
        return Err(error);
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WidgetClickLog {
    source: String,
    x: f64,
    y: f64,
    button: i32,
    target: String,
    width: f64,
    height: f64,
}

/// 记录任务栏组件点击位置及命中元素，用于调试实际可点击区域。
#[tauri::command]
fn log_widget_click(payload: WidgetClickLog, state: tauri::State<'_, WidgetState>) {
    let message = format!(
        "任务栏窗口点击 source={} button={} x={:.1} y={:.1} window={:.1}x{:.1} target={}",
        payload.source,
        payload.button,
        payload.x,
        payload.y,
        payload.width,
        payload.height,
        payload.target.replace('\r', " ").replace('\n', " ")
    );
    println!("{message}");
    append_info_log(&state.log_path, &message);
}

/// 周期性提升任务栏组件 Z-order，并记录鼠标实际命中的原生 HWND。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct NativeTaskbarPoint {
    x: f64,
    y: f64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct NativeTaskbarClick {
    x: f64,
    y: f64,
    region_id: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct NativeTaskbarPointerPoll {
    cursor: Option<NativeTaskbarPoint>,
    cursor_region_id: Option<String>,
    left_button_down: bool,
    left_downs: Vec<NativeTaskbarClick>,
    right_clicks: Vec<NativeTaskbarClick>,
}

#[tauri::command]
fn poll_taskbar_native_pointer(state: tauri::State<'_, WidgetState>) -> NativeTaskbarPointerPoll {
    #[cfg(target_os = "windows")]
    {
        for message in taskbar::take_native_events() {
            println!("[{}] {message}", local_log_timestamp());
            append_info_log(&state.log_path, &message);
        }
        let cursor = taskbar::native_cursor_position()
            .map(|point| NativeTaskbarPoint { x: point.x, y: point.y });
        let cursor_region_id = taskbar::native_cursor_region_id();
        let left_downs = taskbar::take_native_left_downs()
            .into_iter()
            .map(|point| NativeTaskbarClick { x: point.x, y: point.y, region_id: point.region_id })
            .collect();
        let right_clicks = taskbar::take_native_right_clicks()
            .into_iter()
            .map(|point| NativeTaskbarClick { x: point.x, y: point.y, region_id: point.region_id })
            .collect();
        let left_button_down = taskbar::native_left_button_down();
        return NativeTaskbarPointerPoll { cursor, cursor_region_id, left_button_down, left_downs, right_clicks };
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = state;
        NativeTaskbarPointerPoll { cursor: None, cursor_region_id: None, left_button_down: false, left_downs: Vec::new(), right_clicks: Vec::new() }
    }
}

#[tauri::command]
fn log_taskbar_input_debug(message: String, state: tauri::State<'_, WidgetState>) {
    println!("[{}] {message}", local_log_timestamp());
    append_info_log(&state.log_path, &message);
}

#[tauri::command]
fn probe_taskbar_input(
    app: AppHandle,
    state: tauri::State<'_, WidgetState>,
) -> Result<Option<String>, String> {
    #[cfg(target_os = "windows")]
    {
        let window = app
            .get_webview_window("main")
            .ok_or_else(|| "找不到 main 窗口".to_string())?;
        match taskbar::reinforce_and_probe(&window) {
            Ok(Some(message)) => {
                println!("[{}] {message}", local_log_timestamp());
                append_info_log(&state.log_path, &message);
                Ok(Some(message))
            }
            Ok(None) => Ok(None),
            Err(error) => {
                append_error_log(&state.log_path, &format!("任务栏原生命中诊断失败：{error}"));
                Err(error)
            }
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = (app, state);
        Ok(None)
    }
}

#[tauri::command]
fn get_taskbar_test_background(state: tauri::State<'_, WidgetState>) -> bool {
    state.taskbar_test_background.lock().map(|value| *value).unwrap_or(false)
}

#[tauri::command]
fn set_taskbar_test_background(
    app: AppHandle,
    state: tauri::State<'_, WidgetState>,
    visible: bool,
) -> Result<(), String> {
    *state
        .taskbar_test_background
        .lock()
        .map_err(|error| format!("无法锁定任务栏测试背景状态：{error}"))? = visible;
    app.emit("taskbar-test-background-changed", visible)
        .map_err(|error| format!("无法切换任务栏测试背景：{error}"))?;
    append_info_log(&state.log_path, &format!("taskbar-test-background visible={visible}"));
    Ok(())
}

const STARTUP_REGISTRY_PATH: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
const STARTUP_VALUE_NAME: &str = "TBWidget";

#[cfg(target_os = "windows")]
fn current_startup_command() -> Result<String, String> {
    let executable = env::current_exe()
        .map_err(|error| format!("无法读取当前程序路径：{error}"))?;
    Ok(format!("\"{}\"", executable.display()))
}

/// 查询当前用户是否已启用开机启动。
#[tauri::command]
fn get_startup_enabled() -> Result<bool, String> {
    #[cfg(target_os = "windows")]
    {
        let output = Command::new("reg.exe")
            .args(["query", STARTUP_REGISTRY_PATH, "/v", STARTUP_VALUE_NAME])
            .output()
            .map_err(|error| format!("无法查询开机启动项：{error}"))?;
        if !output.status.success() {
            return Ok(false);
        }
        let text = String::from_utf8_lossy(&output.stdout);
        return Ok(text.contains(STARTUP_VALUE_NAME));
    }

    #[cfg(not(target_os = "windows"))]
    Ok(false)
}

/// 设置当前用户开机启动。
#[tauri::command]
fn set_startup_enabled(
    state: tauri::State<'_, WidgetState>,
    enabled: bool,
) -> Result<bool, String> {
    #[cfg(target_os = "windows")]
    {
        let status = if enabled {
            let command = current_startup_command()?;
            Command::new("reg.exe")
                .args([
                    "add",
                    STARTUP_REGISTRY_PATH,
                    "/v",
                    STARTUP_VALUE_NAME,
                    "/t",
                    "REG_SZ",
                    "/d",
                    &command,
                    "/f",
                ])
                .status()
                .map_err(|error| format!("无法写入开机启动项：{error}"))?
        } else {
            Command::new("reg.exe")
                .args(["delete", STARTUP_REGISTRY_PATH, "/v", STARTUP_VALUE_NAME, "/f"])
                .status()
                .map_err(|error| format!("无法删除开机启动项：{error}"))?
        };
        if !status.success() && enabled {
            return Err("写入开机启动项失败".to_string());
        }
        append_info_log(
            &state.log_path,
            &format!("开机启动已{}", if enabled { "启用" } else { "关闭" }),
        );
        return get_startup_enabled();
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = (state, enabled);
        Err("当前平台暂不支持开机启动设置".to_string())
    }
}

/// 退出整个组件应用。
#[tauri::command]
fn quit_application(app: AppHandle) {
    app.exit(0);
}

/// 创建系统托盘图标：左键直接打开设置，右键显示设置/退出菜单。
///
/// # Errors
/// 当图标或托盘创建失败时返回中文错误信息。
fn setup_tray(app: &AppHandle, _config: &WidgetConfig) -> Result<TrayIcon, String> {
    let icon = app
        .default_window_icon()
        .cloned()
        .ok_or_else(|| "无法获取默认窗口图标".to_string())?;
    let settings_item = MenuItemBuilder::with_id("settings", "设置")
        .build(app)
        .map_err(|error| format!("无法创建托盘设置菜单：{error}"))?;
    let separator = PredefinedMenuItem::separator(app)
        .map_err(|error| format!("无法创建托盘菜单分隔线：{error}"))?;
    let quit_item = MenuItemBuilder::with_id("quit", "退出")
        .build(app)
        .map_err(|error| format!("无法创建托盘退出菜单：{error}"))?;
    let menu = MenuBuilder::new(app)
        .items(&[&settings_item, &separator, &quit_item])
        .build()
        .map_err(|error| format!("无法创建托盘菜单：{error}"))?;

    TrayIconBuilder::new()
        .icon(icon)
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app_handle, event| match event.id().as_ref() {
            "settings" => show_settings_window(app_handle),
            "quit" => app_handle.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click { button, button_state, .. } = event {
                if button_state != MouseButtonState::Up || button != MouseButton::Left {
                    return;
                }
                let app_handle = tray.app_handle();
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
    let plugins_dir = data_dir.join("plugins");
    let plugin_config_path = data_dir.join("plugin-config.json");
    fs::write(&log_path, "").map_err(|error| {
        std::io::Error::other(format!(
            "无法在每次启动时重建组件日志文件 {}：{error}",
            log_path.display()
        ))
    })?;
    init_history_database(&db_path).map_err(std::io::Error::other)?;
    let config = load_widget_config(&db_path);
    app.manage(WidgetState {
        config: Mutex::new(config.clone()),
        log_path,
        db_path,
        plugins_dir,
        plugin_config_path,
        plugin_popup: Mutex::new(None),
        plugin_popup_sizes: Mutex::new(BTreeMap::new()),
        plugin_settings: Mutex::new(None),
        tray_icon: Mutex::new(None),
        taskbar_test_background: Mutex::new(false),
        system_monitor: Mutex::new(system_monitor::SystemMonitorSampler::new()),
    });
    if let Some(popup) = app.get_webview_window("plugin-popup") {
        let popup_for_event = popup.clone();
        let app_handle = app.handle().clone();
        popup.on_window_event(move |event| {
            if !matches!(event, tauri::WindowEvent::Focused(false)) {
                return;
            }
            let state = app_handle.state::<WidgetState>();
            if let Ok(mut active) = state.plugin_popup.lock() {
                *active = None;
            }
            if let Err(error) = popup_for_event.hide() {
                eprintln!("插件弹窗失焦隐藏失败：{error}");
            }
        });
    }
    let tray_icon = setup_tray(app.handle(), &config).map_err(std::io::Error::other)?;
    match app.state::<WidgetState>().tray_icon.lock() {
        Ok(mut state_tray_icon) => *state_tray_icon = Some(tray_icon),
        Err(error) => eprintln!("无法保存托盘图标状态：{error}"),
    }
    Ok(())
}

/// 在 Tauri 就绪后将主窗口挂载到任务栏。
fn handle_run_event(app_handle: &AppHandle, event: tauri::RunEvent) {
    if let tauri::RunEvent::Ready = event {
        let Some(window) = app_handle.get_webview_window("main") else {
            eprintln!("找不到 main 窗口");
            return;
        };

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

            // main WebView 始终原生点透。插件/按钮点击由 WH_MOUSE_LL 在已上报的
            // 交互矩形内拦截并转发；空白区域继续交给 Windows 任务栏。
            if let Err(error) = window.set_ignore_cursor_events(true) {
                eprintln!("无法启用任务栏空白区域点透：{error}");
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

/// 创建 Tauri 应用，注册底座窗口、插件基础能力和托盘入口。
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(setup_app)
        .invoke_handler(tauri::generate_handler![
            set_widget_width,
            set_taskbar_interactive_regions,
            get_taskbar_right_reserved_width,
            get_widget_config,
            save_widget_settings,
            get_taskbar_test_background,
            set_taskbar_test_background,
            get_startup_enabled,
            set_startup_enabled,
            hide_settings_window,
            resize_settings_window,
            open_widget_log,
            get_widget_log_path,
            log_widget_click,
            poll_taskbar_native_pointer,
            log_taskbar_input_debug,
            probe_taskbar_input,
            quit_application,
            plugins::list_plugins,
            plugins::refresh_plugins,
            plugins::set_plugin_enabled,
            plugins::set_plugin_order,
            plugins::show_plugin_settings_window,
            plugins::resize_plugin_settings_window,
            plugins::hide_plugin_settings_window,
            plugins::get_active_plugin_settings_id,
            plugins::show_plugin_popup,
            plugins::hide_plugin_popup,
            plugins::resize_plugin_popup,
            plugins::get_active_plugin_popup_id,
            plugins::plugin_http_request,
            plugins::plugin_fs_read_text,
            plugins::plugin_fs_read_bytes,
            plugins::plugin_storage_get,
            plugins::plugin_storage_set,
            plugins::plugin_storage_remove,
            plugins::plugin_storage_clear,
            plugins::plugin_system_metrics
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(handle_run_event);
}
