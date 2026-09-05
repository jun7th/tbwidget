use std::{
    collections::BTreeMap,
    env, fs,
    fs::OpenOptions,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};

#[cfg(feature = "sqlite-storage")]
use rusqlite::Connection;
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
mod plugin_assets;
mod system_monitor;


#[cfg(feature = "sqlite-storage")]
const DATABASE_FILE_NAME: &str = "widget-history.sqlite";
const CONFIG_FILE_NAME: &str = "config.json";
const SAVE_DIR_NAME: &str = "save";
const CONFIG_DIR_NAME: &str = "config";
const LOG_DIR_NAME: &str = "log";
const LEGACY_WIDGET_CONFIG_FILE_NAME: &str = "widget-config.json";
const LEGACY_PLUGIN_CONFIG_FILE_NAME: &str = "plugin-config.json";
const LEGACY_PLUGIN_STORAGE_FILE_NAME: &str = "plugin-storage.json";
const PLUGINS_DIR_NAME: &str = "plugins";
const SETTINGS_MARGIN: f64 = 8.0;
const SETTINGS_MIN_WIDTH: f64 = 320.0;
static RUNTIME_DEBUG_LOGS: AtomicBool = AtomicBool::new(false);

fn should_write_log(debug_build: bool, runtime_debug: bool, level: &str) -> bool {
    debug_build
        || runtime_debug
        || matches!(level, "FLOW" | "ACTION" | "ERROR" | "PLUGIN")
}

pub(crate) fn debug_logging_enabled() -> bool {
    cfg!(debug_assertions) || RUNTIME_DEBUG_LOGS.load(Ordering::Relaxed)
}

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

    #[test]
    fn release_logging_keeps_persistent_categories() {
        assert!(should_write_log(false, false, "FLOW"));
        assert!(should_write_log(false, false, "ACTION"));
        assert!(should_write_log(false, false, "ERROR"));
        assert!(!should_write_log(false, false, "DEBUG"));
        assert!(!should_write_log(false, false, "INFO"));
        assert!(should_write_log(false, false, "PLUGIN"));
    }

    #[test]
    fn runtime_debug_enables_all_log_categories() {
        assert!(should_write_log(false, true, "DEBUG"));
        assert!(should_write_log(false, true, "INFO"));
    }

    #[test]
    fn debug_build_enables_all_log_categories() {
        assert!(should_write_log(true, false, "DEBUG"));
        assert!(should_write_log(true, false, "INFO"));
    }

}

struct WidgetState {
    config: Mutex<WidgetConfig>,
    log_path: PathBuf,
    config_path: PathBuf,
    config_file_lock: Mutex<()>,
    plugins_dir: PathBuf,
    plugin_popup: Mutex<Option<PluginPopupAnchor>>,
    plugin_popup_sizes: Mutex<BTreeMap<String, (f64, f64)>>,
    plugin_settings: Mutex<Option<String>>,
    tray_icon: Mutex<Option<TrayIcon>>,
    taskbar_test_background: Mutex<bool>,
    plugins_ready: AtomicBool,
    system_monitor: Arc<Mutex<Option<system_monitor::SystemMonitorSampler>>>,
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

/// 获取当天日志文件名使用的本地日期 YYYYMMDD。
#[cfg(target_os = "windows")]
fn local_log_date() -> String {
    let mut local_time: SYSTEMTIME = unsafe { std::mem::zeroed() };
    unsafe { GetLocalTime(&mut local_time) };
    format!("{:04}{:02}{:02}", local_time.wYear, local_time.wMonth, local_time.wDay)
}

#[cfg(not(target_os = "windows"))]
fn local_log_date() -> String {
    "00000000".to_string()
}

/// 将一条结构化运行日志追加到文件。日志只追加，不覆盖、不滚动。
pub(crate) fn append_log(path: &Path, level: &str, module: &str, message: &str) {
    if !should_write_log(
        cfg!(debug_assertions),
        RUNTIME_DEBUG_LOGS.load(Ordering::Relaxed),
        level,
    ) {
        return;
    }
    let timestamp = local_log_timestamp();
    let result = path
        .parent()
        .map(fs::create_dir_all)
        .transpose()
        .and_then(|_| OpenOptions::new().create(true).append(true).open(path))
        .and_then(|mut file| writeln!(file, "[{timestamp}] {level:<5} [{module}] {message}"));
    if let Err(error) = result {
        eprintln!("无法写入组件日志文件 {}：{error}", path.display());
    }
}

/// 将错误消息追加到组件日志文件。
pub(crate) fn append_error_log(path: &Path, message: &str) {
    append_log(path, "ERROR", "app", message);
}

/// 将普通运行信息追加到组件日志文件。
pub(crate) fn append_debug_log(path: &Path, message: &str) {
    append_log(path, "DEBUG", "app", message);
}

#[cfg(target_os = "windows")]
fn flush_taskbar_native_events(path: &Path) {
    for message in taskbar::take_native_events() {
        append_log(path, "DEBUG", "taskbar-native", &message);
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

/// 初始化本地 SQLite 持续数据数据库。
///
/// SQLite 不再保存应用设置、插件配置或插件 storage；这里只保留数据库文件，
/// 供历史记录、采样、缓存等持续数据使用。已有旧表不会删除，避免破坏用户数据。
#[cfg(feature = "sqlite-storage")]
fn init_history_database(path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("无法创建数据库目录 {}：{error}", parent.display()))?;
    }
    Connection::open(path)
        .map(|_| ())
        .map_err(|error| format!("无法打开持续数据数据库 {}：{error}", path.display()))
}

#[cfg(feature = "sqlite-storage")]
fn cleanup_legacy_settings_tables(path: &Path, log_path: &Path) -> Result<(), String> {
    let connection = Connection::open(path)
        .map_err(|error| format!("无法打开持续数据数据库 {}：{error}", path.display()))?;
    connection
        .execute_batch(
            "DROP TABLE IF EXISTS widget_config;
             DROP TABLE IF EXISTS plugin_storage;",
        )
        .map_err(|error| format!("无法清理 SQLite 旧设置表：{error}"))?;
    append_log(
        log_path,
        "FLOW",
        "migration",
        "SQLite 旧设置表 widget_config/plugin_storage 已清理，SQLite 仅保留给持续数据",
    );
    Ok(())
}

fn read_config_root(path: &Path) -> Result<serde_json::Value, String> {
    match fs::read_to_string(path) {
        Ok(content) => {
            let value: serde_json::Value = serde_json::from_str(&content)
                .map_err(|error| format!("配置 JSON {} 已损坏：{error}", path.display()))?;
            if !value.is_object() {
                return Err(format!("配置 JSON {} 根节点必须是对象", path.display()));
            }
            Ok(value)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(serde_json::json!({ "debug": false, "widget": WidgetConfig::default(), "plugins": {} }))
        }
        Err(error) => Err(format!("无法读取配置 {}：{error}", path.display())),
    }
}

fn config_debug_enabled(path: &Path) -> bool {
    read_config_root(path)
        .ok()
        .and_then(|root| root.get("debug").and_then(serde_json::Value::as_bool))
        .unwrap_or(false)
}

#[tauri::command]
fn get_debug_mode(state: tauri::State<'_, WidgetState>) -> bool {
    config_debug_enabled(&state.config_path)
}

fn write_config_root(path: &Path, value: &serde_json::Value) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("无法创建配置目录 {}：{error}", parent.display()))?;
    }
    let content = serde_json::to_string_pretty(value)
        .map_err(|error| format!("无法序列化配置：{error}"))?;
    fs::write(path, format!("{content}\n"))
        .map_err(|error| format!("无法写入配置 {}：{error}", path.display()))
}

fn save_widget_config(path: &Path, config: &WidgetConfig) -> Result<(), String> {
    let mut root = read_config_root(path)?;
    let object = root
        .as_object_mut()
        .ok_or_else(|| "配置根节点必须是对象".to_string())?;
    object.insert(
        "widget".to_string(),
        serde_json::to_value(config).map_err(|error| format!("无法序列化组件配置：{error}"))?,
    );
    write_config_root(path, &root)
}

#[cfg(feature = "sqlite-storage")]
fn load_legacy_widget_config(db_path: &Path) -> Option<WidgetConfig> {
    let connection = Connection::open(db_path).ok()?;
    let content = connection
        .query_row(
            "SELECT content FROM widget_config WHERE id = 1",
            [],
            |row| row.get::<_, String>(0),
        )
        .ok()?;
    serde_json::from_str(&content).ok()
}

fn merge_legacy_plugin_runtime(root: &mut serde_json::Value, legacy: &serde_json::Value) {
    let Some(legacy_plugins) = legacy.get("plugins").and_then(serde_json::Value::as_object) else {
        return;
    };
    let plugins = root
        .as_object_mut()
        .expect("配置根节点已验证")
        .entry("plugins")
        .or_insert_with(|| serde_json::json!({}));
    if !plugins.is_object() {
        *plugins = serde_json::json!({});
    }
    let plugins = plugins.as_object_mut().expect("plugins 已转换为对象");
    for (id, legacy_entry) in legacy_plugins {
        let entry = plugins.entry(id.clone()).or_insert_with(|| serde_json::json!({}));
        if !entry.is_object() {
            *entry = serde_json::json!({});
        }
        let entry = entry.as_object_mut().expect("插件配置已转换为对象");
        for key in ["enabled", "order"] {
            if let Some(value) = legacy_entry.get(key) {
                entry.insert(key.to_string(), value.clone());
            }
        }
    }
}

fn merge_legacy_plugin_storage(root: &mut serde_json::Value, legacy: &serde_json::Value) {
    let Some(legacy_plugins) = legacy.get("plugins").and_then(serde_json::Value::as_object) else {
        return;
    };
    let plugins = root
        .as_object_mut()
        .expect("配置根节点已验证")
        .entry("plugins")
        .or_insert_with(|| serde_json::json!({}));
    if !plugins.is_object() {
        *plugins = serde_json::json!({});
    }
    let plugins = plugins.as_object_mut().expect("plugins 已转换为对象");
    for (id, legacy_storage) in legacy_plugins {
        let entry = plugins.entry(id.clone()).or_insert_with(|| serde_json::json!({}));
        if !entry.is_object() {
            *entry = serde_json::json!({});
        }
        let entry = entry.as_object_mut().expect("插件配置已转换为对象");
        entry.insert("storage".to_string(), legacy_storage.clone());
    }
}

#[cfg(feature = "sqlite-storage")]
fn load_legacy_plugin_storage_from_sqlite(db_path: &Path, log_path: &Path) -> serde_json::Value {
    let mut result = serde_json::json!({ "plugins": {} });
    let Ok(connection) = Connection::open(db_path) else { return result; };
    let Ok(mut statement) = connection.prepare(
        "SELECT plugin_id, storage_key, json_value FROM plugin_storage ORDER BY plugin_id, storage_key",
    ) else { return result; };
    let Ok(rows) = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    }) else { return result; };

    let mut count = 0usize;
    for row in rows.flatten() {
        let (plugin_id, key, content) = row;
        match serde_json::from_str::<serde_json::Value>(&content) {
            Ok(value) => {
                let plugins = result
                    .get_mut("plugins")
                    .and_then(serde_json::Value::as_object_mut)
                    .expect("plugins migration object");
                let plugin = plugins
                    .entry(plugin_id)
                    .or_insert_with(|| serde_json::json!({}));
                if !plugin.is_object() {
                    *plugin = serde_json::json!({});
                }
                plugin
                    .as_object_mut()
                    .expect("plugin migration storage object")
                    .insert(key, value);
                count += 1;
            }
            Err(error) => append_log(
                log_path,
                "ERROR",
                "migration",
                &format!("跳过损坏的旧 SQLite 插件 storage JSON：{error}"),
            ),
        }
    }
    if count > 0 {
        append_log(log_path, "FLOW", "migration", &format!("从旧 SQLite 读取插件配置键数={count}"));
    }
    result
}

fn initialize_unified_config(
    config_path: &Path,
    bundled_config_path: Option<&Path>,
    legacy_widget_path: &Path,
    legacy_plugin_config_path: &Path,
    legacy_plugin_storage_path: &Path,
    legacy_db_path: Option<&Path>,
    log_path: &Path,
) -> Result<WidgetConfig, String> {
    if config_path.is_file() {
        let root = read_config_root(config_path)?;
        let widget = root
            .get("widget")
            .cloned()
            .map(serde_json::from_value::<WidgetConfig>)
            .transpose()
            .map_err(|error| format!("组件配置无效：{error}"))?
            .unwrap_or_default();
        append_log(log_path, "FLOW", "config", &format!("已加载主配置 {}", config_path.display()));
        return Ok(widget);
    }

    let mut root = if let Some(path) = bundled_config_path.filter(|path| path.is_file()) {
        read_config_root(path)?
    } else {
        default_config_root()?
    };

    if let Ok(content) = fs::read_to_string(legacy_widget_path) {
        if let Ok(widget) = serde_json::from_str::<WidgetConfig>(&content) {
            root["widget"] = serde_json::to_value(widget)
                .map_err(|error| format!("无法迁移旧组件配置：{error}"))?;
            append_log(log_path, "FLOW", "migration", "旧 widget-config.json 已合并到 config.json");
        }
    }

    // SQLite 迁移代码保留，但默认 feature 未启用时完全不编译、不读取数据库。
    #[cfg(feature = "sqlite-storage")]
    if !legacy_widget_path.is_file() {
        if let Some(widget) = legacy_db_path.and_then(load_legacy_widget_config) {
            root["widget"] = serde_json::to_value(widget)
                .map_err(|error| format!("无法迁移旧 SQLite 组件配置：{error}"))?;
            append_log(log_path, "FLOW", "migration", "旧 SQLite 组件配置已合并到 config.json");
        }
    }

    if let Ok(content) = fs::read_to_string(legacy_plugin_config_path) {
        if let Ok(legacy) = serde_json::from_str::<serde_json::Value>(&content) {
            merge_legacy_plugin_runtime(&mut root, &legacy);
            append_log(log_path, "FLOW", "migration", "旧 plugin-config.json 已合并到 config.json");
        }
    }

    if let Ok(content) = fs::read_to_string(legacy_plugin_storage_path) {
        if let Ok(legacy) = serde_json::from_str::<serde_json::Value>(&content) {
            merge_legacy_plugin_storage(&mut root, &legacy);
            append_log(log_path, "FLOW", "migration", "旧 plugin-storage.json 已合并到 config.json");
        }
    }

    #[cfg(feature = "sqlite-storage")]
    if !legacy_plugin_storage_path.is_file() {
        if let Some(db_path) = legacy_db_path {
            let legacy = load_legacy_plugin_storage_from_sqlite(db_path, log_path);
            merge_legacy_plugin_storage(&mut root, &legacy);
        }
    }

    #[cfg(not(feature = "sqlite-storage"))]
    let _ = legacy_db_path;

    write_config_root(config_path, &root)?;
    append_log(log_path, "FLOW", "config", &format!("主配置已写入 {}", config_path.display()));

    for path in [legacy_widget_path, legacy_plugin_config_path, legacy_plugin_storage_path] {
        if path.is_file() {
            match fs::remove_file(path) {
                Ok(()) => append_log(log_path, "FLOW", "migration", &format!("已删除旧配置文件 {}", path.display())),
                Err(error) => append_log(log_path, "ERROR", "migration", &format!("删除旧配置文件 {} 失败：{error}", path.display())),
            }
        }
    }

    root.get("widget")
        .cloned()
        .map(serde_json::from_value::<WidgetConfig>)
        .transpose()
        .map_err(|error| format!("组件配置无效：{error}"))
        .map(|config| config.unwrap_or_default())
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

/// 将插件 iframe 内的运行日志写入统一日志文件。
#[tauri::command]
fn log_plugin_runtime(
    state: tauri::State<'_, WidgetState>,
    plugin_id: String,
    surface: String,
    level: String,
    message: String,
) {
    let plugin_id = plugin_id.replace('\r', " ").replace('\n', " ");
    let surface = surface.replace('\r', " ").replace('\n', " ");
    let level = level.replace('\r', " ").replace('\n', " ");
    let message = message
        .replace('\r', " ")
        .replace('\n', " ")
        .chars()
        .take(8192)
        .collect::<String>();
    append_log(
        &state.log_path,
        "PLUGIN",
        "plugin-runtime",
        &format!(
            "plugin={} surface={} level={} {}",
            plugin_id, surface, level, message
        ),
    );
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
    let _guard = state
        .config_file_lock
        .lock()
        .map_err(|error| format!("无法锁定统一配置文件：{error}"))?;
    if let Err(error) = save_widget_config(&state.config_path, &config) {
        append_log(
            &state.log_path,
            "ERROR",
            "config",
            &format!("组件设置修改失败：{error}"),
        );
        return Err(error);
    }
    append_log(&state.log_path, "ACTION", "config", "组件设置修改成功并写入 save/config/config.json");
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
    if let Err(error) = window.set_always_on_top(false) {
        eprintln!("无法取消设置窗口置顶：{error}");
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
    if debug_logging_enabled() {
        println!("{message}");
    }
    append_debug_log(&state.log_path, &message);
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
            append_debug_log(&state.log_path, &message);
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
    if debug_logging_enabled() {
        println!("[{}] {message}", local_log_timestamp());
    }
    append_debug_log(&state.log_path, &message);
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
                if debug_logging_enabled() {
                    println!("[{}] {message}", local_log_timestamp());
                }
                append_debug_log(&state.log_path, &message);
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
fn log_taskbar_display_snapshot(
    _app: AppHandle,
    _state: tauri::State<'_, WidgetState>,
    _stage: String,
) -> Result<(), String> {
    // 保留命令兼容旧前端，但关闭高频 [taskbar-display] diag 快照日志。
    Ok(())
}

#[tauri::command]
fn log_taskbar_ui_debug(message: String, state: tauri::State<'_, WidgetState>) {
    append_log(&state.log_path, "DEBUG", "taskbar-ui", &message);
}

#[tauri::command]
fn log_frontend_error(message: String, state: tauri::State<'_, WidgetState>) {
    append_log(&state.log_path, "ERROR", "frontend", &message);
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
    append_debug_log(&state.log_path, &format!("taskbar-test-background visible={visible}"));
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
        append_log(
            &state.log_path,
            "ACTION",
            "startup",
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

fn default_config_root() -> Result<serde_json::Value, String> {
    let value: serde_json::Value = serde_json::from_str(include_str!("../default-config.json"))
        .map_err(|error| format!("内置默认配置无效：{error}"))?;
    if !value.is_object() {
        return Err("内置默认配置根节点必须是对象".to_string());
    }
    Ok(value)
}

/// 运行根目录：开发模式为工程根目录；发布版严格使用当前 EXE 所在目录。
fn runtime_root_dir() -> Result<PathBuf, String> {
    if cfg!(debug_assertions) {
        return PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| "无法确定 tbwidget 工程根目录".to_string());
    }
    env::current_exe()
        .map_err(|error| format!("无法获取当前可执行文件路径：{error}"))?
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| "无法确定当前 EXE 所在目录".to_string())
}

fn copy_file_if_missing(source: &Path, destination: &Path) -> Result<bool, String> {
    if destination.exists() || !source.is_file() || source == destination {
        return Ok(false);
    }
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("无法创建迁移目录 {}：{error}", parent.display()))?;
    }
    fs::copy(source, destination)
        .map(|_| true)
        .map_err(|error| format!("无法迁移文件 {} -> {}：{error}", source.display(), destination.display()))
}

/// 初始化 JSON 配置、SQLite 持续数据、插件资源、追加日志和系统托盘。
fn setup_app(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let runtime_root = runtime_root_dir().map_err(std::io::Error::other)?;
    fs::create_dir_all(&runtime_root).map_err(|error| {
        std::io::Error::other(format!("无法创建运行目录 {}：{error}", runtime_root.display()))
    })?;

    let save_dir = runtime_root.join(SAVE_DIR_NAME);
    let config_dir = save_dir.join(CONFIG_DIR_NAME);
    let plugin_config_dir = config_dir.join("plugin");
    let log_dir = save_dir.join(LOG_DIR_NAME);
    fs::create_dir_all(&plugin_config_dir).map_err(|error| {
        std::io::Error::other(format!("无法创建配置目录 {}：{error}", plugin_config_dir.display()))
    })?;
    fs::create_dir_all(&log_dir).map_err(|error| {
        std::io::Error::other(format!("无法创建日志目录 {}：{error}", log_dir.display()))
    })?;

    let log_path = log_dir.join(format!("{}.log", local_log_date()));
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .map_err(|error| std::io::Error::other(format!("无法打开追加日志 {}：{error}", log_path.display())))?;

    let config_path = config_dir.join(CONFIG_FILE_NAME);
    RUNTIME_DEBUG_LOGS.store(config_debug_enabled(&config_path), Ordering::Relaxed);

    append_log(&log_path, "FLOW", "startup", "================ 程序启动 ================");
    append_log(&log_path, "FLOW", "startup", &format!("版本={}", app.package_info().version));
    append_log(&log_path, "DEBUG", "startup", &format!("debug_build={}", cfg!(debug_assertions)));
    append_log(&log_path, "DEBUG", "startup", &format!("运行根目录={}", runtime_root.display()));
    if let Ok(executable) = env::current_exe() {
        append_log(&log_path, "DEBUG", "startup", &format!("可执行文件={}", executable.display()));
    }

    let plugins_dir = runtime_root.join(PLUGINS_DIR_NAME);
    match app.path().resource_dir() {
        Ok(resource_dir) => {
            append_log(&log_path, "DEBUG", "startup", &format!("Tauri资源目录={}", resource_dir.display()));
            append_log(
                &log_path,
                "DEBUG",
                "plugins",
                &format!("打包插件资源路径={}", resource_dir.join(PLUGINS_DIR_NAME).display()),
            );
        }
        Err(error) => append_log(&log_path, "ERROR", "startup", &format!("无法解析Tauri资源目录：{error}")),
    }

    // SQLite 功能暂时屏蔽。代码和依赖 feature 均保留，需要时可用
    // `cargo build --features sqlite-storage` 恢复编译。默认构建不会创建/读取 SQLite。
    #[cfg(feature = "sqlite-storage")]
    let db_path = runtime_root.join(DATABASE_FILE_NAME);
    #[cfg(feature = "sqlite-storage")]
    let legacy_db_path = Some(db_path.as_path());
    #[cfg(not(feature = "sqlite-storage"))]
    let legacy_db_path: Option<&Path> = None;

    // 兼容上一版：如果 EXE 同目录有统一 config.json，首次运行迁移到 save/config/config.json。
    let legacy_unified_config = runtime_root.join(CONFIG_FILE_NAME);
    if !config_path.is_file() && legacy_unified_config.is_file() {
        if let Err(error) = copy_file_if_missing(&legacy_unified_config, &config_path) {
            append_log(&log_path, "ERROR", "migration", &format!("旧 config.json 迁移失败：{error}"));
        } else {
            append_log(&log_path, "FLOW", "migration", &format!("旧 config.json 已迁移到 {}", config_path.display()));
            if let Err(error) = fs::remove_file(&legacy_unified_config) {
                append_log(&log_path, "ERROR", "migration", &format!("旧 config.json 删除失败：{error}"));
            }
        }
    }

    #[cfg(feature = "sqlite-storage")]
    {
        init_history_database(&db_path).map_err(std::io::Error::other)?;
        append_log(&log_path, "DEBUG", "database", &format!("SQLite 持续数据文件={}", db_path.display()));
    }
    #[cfg(not(feature = "sqlite-storage"))]
    append_log(&log_path, "DEBUG", "database", "SQLite 功能当前已屏蔽");

    append_log(&log_path, "DEBUG", "plugins", &format!("插件目录={}", plugins_dir.display()));

    // 插件目录会在 WidgetState 建立后扫描；新发现的插件登记为默认关闭。
    if !plugins_dir.is_dir() {
        fs::create_dir_all(&plugins_dir).map_err(|error| {
            std::io::Error::other(format!("无法创建插件目录 {}：{error}", plugins_dir.display()))
        })?;
        append_log(&log_path, "FLOW", "plugins", &format!("插件目录不存在，已创建 {}", plugins_dir.display()));
    }

    let legacy_widget_config_path = runtime_root.join(LEGACY_WIDGET_CONFIG_FILE_NAME);
    let legacy_plugin_config_path = runtime_root.join(LEGACY_PLUGIN_CONFIG_FILE_NAME);
    let legacy_plugin_storage_path = runtime_root.join(LEGACY_PLUGIN_STORAGE_FILE_NAME);
    let config = initialize_unified_config(
        &config_path,
        None,
        &legacy_widget_config_path,
        &legacy_plugin_config_path,
        &legacy_plugin_storage_path,
        legacy_db_path,
        &log_path,
    )
    .map_err(std::io::Error::other)?;

    let config_debug = config_debug_enabled(&config_path);
    RUNTIME_DEBUG_LOGS.store(config_debug, Ordering::Relaxed);
    append_log(&log_path, "FLOW", "config", &format!("config.debug={config_debug}"));

    // initialize_unified_config 在配置不存在时一定写出 config.json。
    if !config_path.is_file() {
        write_config_root(
            &config_path,
            &serde_json::json!({ "debug": false, "widget": WidgetConfig::default(), "plugins": {} }),
        )
        .map_err(std::io::Error::other)?;
        append_log(&log_path, "FLOW", "config", &format!("配置不存在，已创建 {}", config_path.display()));
    }

    // 必须在 config_path/log_path 移入 WidgetState 前完成；避免 PathBuf move 后再次借用。
    #[cfg(feature = "sqlite-storage")]
    if config_path.is_file() {
        if let Err(error) = cleanup_legacy_settings_tables(&db_path, &log_path) {
            append_log(&log_path, "ERROR", "migration", &error);
        }
    }

    app.manage(WidgetState {
        config: Mutex::new(config.clone()),
        log_path,
        config_path,
        config_file_lock: Mutex::new(()),
        plugins_dir,
        plugin_popup: Mutex::new(None),
        plugin_popup_sizes: Mutex::new(BTreeMap::new()),
        plugin_settings: Mutex::new(None),
        tray_icon: Mutex::new(None),
        taskbar_test_background: Mutex::new(false),
        plugins_ready: AtomicBool::new(false),
        // 系统监控采样器首次真正请求时再初始化，避免 System::new_all()
        // 阻塞应用启动和任务栏窗口首次显示。
        system_monitor: Arc::new(Mutex::new(None)),
    });

    {
        let state = app.state::<WidgetState>();
        if let Err(error) = plugins::migrate_embedded_plugin_storage(state.inner()) {
            append_log(&state.log_path, "ERROR", "migration", &format!("拆分插件配置失败：{error}"));
        }
        if let Err(error) = plugins::scan_plugins_on_startup(state.inner()) {
            append_log(&state.log_path, "ERROR", "plugins", &format!("启动插件扫描失败：{error}"));
        }
        if let Err(error) = plugins::ensure_registered_plugin_configs(state.inner()) {
            append_log(&state.log_path, "ERROR", "plugins", &format!("创建插件配置文件失败：{error}"));
        }
        if let Err(error) = plugins::log_registered_plugins(state.inner()) {
            append_log(&state.log_path, "ERROR", "plugins", &format!("读取已登记插件失败：{error}"));
        }

        // 所有会读写插件主配置的启动步骤完成后再开放 list_plugins/tbplugin://。
        // 前端在 ready 前只会得到“初始化中”，并自动重试，不再让 iframe
        // 撞上启动期配置写入造成第一次空白。
        state.plugins_ready.store(true, Ordering::Release);
        append_log(&state.log_path, "FLOW", "plugins", "插件系统初始化完成");
    }

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
                append_log(&state.log_path, "ERROR", "plugins", &format!("插件弹窗失焦隐藏失败：{error}"));
            }
        });
    }

    let tray_icon = setup_tray(app.handle(), &config).map_err(std::io::Error::other)?;
    match app.state::<WidgetState>().tray_icon.lock() {
        Ok(mut state_tray_icon) => *state_tray_icon = Some(tray_icon),
        Err(error) => eprintln!("无法保存托盘图标状态：{error}"),
    }
    append_log(&app.state::<WidgetState>().log_path, "FLOW", "startup", "程序初始化完成");
    let exe = env::current_exe().map(|path| path.display().to_string()).unwrap_or_else(|error| format!("<current_exe error: {error}>"));
    let cwd = env::current_dir().map(|path| path.display().to_string()).unwrap_or_else(|error| format!("<current_dir error: {error}>"));
    append_log(
        &app.state::<WidgetState>().log_path,
        "DEBUG",
        "startup",
        &format!(
            "runtime build={} exe={} cwd={} config={}",
            if cfg!(debug_assertions) { "debug" } else { "release" },
            exe,
            cwd,
            app.state::<WidgetState>().config_path.display()
        ),
    );
    Ok(())
}

/// 在 Tauri 就绪后将主窗口挂载到任务栏。
fn handle_run_event(app_handle: &AppHandle, event: tauri::RunEvent) {
    if let tauri::RunEvent::Ready = event {
        append_log(&app_handle.state::<WidgetState>().log_path, "FLOW", "startup", "Tauri Ready，开始挂载任务栏窗口");
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
            } else {
                append_log(&app_handle.state::<WidgetState>().log_path, "FLOW", "taskbar", "任务栏窗口挂载成功");
            }

            flush_taskbar_native_events(&app_handle.state::<WidgetState>().log_path);

            // 不再调用 Tauri 的 set_ignore_cursor_events(true)。
            // 日志已确认该 API 会把刚刚 SetParent 到 Shell_TrayWnd 的窗口恢复成顶层窗口，
            // 同时还原 WS_CHILD/WS_VISIBLE 等样式，导致 parent=0、visible=false。
            // 原生 taskbar::attach 已设置 WS_EX_TRANSPARENT，空白区域点透仍由现有 Win32/WH_MOUSE_LL 方案负责。
            append_log(
                &app_handle.state::<WidgetState>().log_path,
                "DEBUG",
                "taskbar",
                "skip set_ignore_cursor_events(true): it resets SetParent/style after taskbar attach",
            );
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
        .register_uri_scheme_protocol("tbplugin", |context, request| plugin_assets::handle(context, request))
        .plugin(tauri_plugin_opener::init())
        .setup(setup_app)
        .invoke_handler(tauri::generate_handler![
            set_widget_width,
            set_taskbar_interactive_regions,
            get_taskbar_right_reserved_width,
            get_widget_config,
            log_plugin_runtime,
            get_debug_mode,
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
            log_taskbar_display_snapshot,
            log_taskbar_ui_debug,
            log_frontend_error,
            quit_application,
            plugins::list_plugins,
            plugins::refresh_plugins,
            plugins::set_plugin_enabled,
            plugins::set_plugin_order,
            plugins::windows::show_plugin_settings_window,
            plugins::windows::present_plugin_settings_window,
            plugins::windows::resize_plugin_settings_window,
            plugins::windows::hide_plugin_settings_window,
            plugins::windows::get_active_plugin_settings_id,
            plugins::windows::show_plugin_popup,
            plugins::windows::hide_plugin_popup,
            plugins::windows::resize_plugin_popup,
            plugins::windows::get_active_plugin_popup_id,
            plugins::host::plugin_http_request,
            plugins::host::plugin_fs_read_text,
            plugins::host::plugin_fs_read_bytes,
            plugins::host::plugin_storage_get,
            plugins::host::plugin_storage_set,
            plugins::host::plugin_storage_remove,
            plugins::host::plugin_storage_clear,
            plugins::host::plugin_system_metrics
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(handle_run_event);
}
