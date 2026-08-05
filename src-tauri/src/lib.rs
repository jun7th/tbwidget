use std::{
    env, fs,
    fs::OpenOptions,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
    sync::Mutex,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sysinfo::{Components, Disks, Networks, System};
#[cfg(target_os = "windows")]
use windows_sys::Win32::{
    Foundation::SYSTEMTIME,
    System::SystemInformation::GetLocalTime,
};
use tauri::{
    menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu},
    tray::TrayIconBuilder,
    AppHandle, Emitter, Manager,
};

#[cfg(target_os = "windows")]
mod taskbar;

const CODEX_USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";
const CODEX_USAGE_PATH: &str = "/backend-api/wham/usage";
const CONFIG_FILE_NAME: &str = "widget-config.json";
const LOG_FILE_NAME: &str = "widget.log";
const ALLOWED_GPT_REFRESH_SECONDS: [u64; 5] = [30, 60, 180, 300, 600];

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
            gpt_refresh_seconds: 60,
            gpt_display_mode: GptDisplayMode::Remaining,
            gpt_proxy_url: String::new(),
        }
    }
}

struct WidgetState {
    config: Mutex<WidgetConfig>,
    config_path: PathBuf,
    log_path: PathBuf,
    system: Mutex<System>,
    components: Mutex<Components>,
    disks: Mutex<Disks>,
    networks: Mutex<Networks>,
    network_instant: Mutex<Instant>,
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

#[derive(Clone)]
struct TrayMenuItems {
    position_left: CheckMenuItem<tauri::Wry>,
    position_right: CheckMenuItem<tauri::Wry>,
    cpu: CheckMenuItem<tauri::Wry>,
    memory: CheckMenuItem<tauri::Wry>,
    gpt: CheckMenuItem<tauri::Wry>,
    gpt_remaining: CheckMenuItem<tauri::Wry>,
    gpt_used: CheckMenuItem<tauri::Wry>,
    temperature: CheckMenuItem<tauri::Wry>,
    disk: CheckMenuItem<tauri::Wry>,
    upload: CheckMenuItem<tauri::Wry>,
    download: CheckMenuItem<tauri::Wry>,
    refresh_30: CheckMenuItem<tauri::Wry>,
    refresh_60: CheckMenuItem<tauri::Wry>,
    refresh_180: CheckMenuItem<tauri::Wry>,
    refresh_300: CheckMenuItem<tauri::Wry>,
    refresh_600: CheckMenuItem<tauri::Wry>,
}

/// 检查 GPT 刷新秒数是否属于允许值。
fn is_valid_refresh_seconds(seconds: u64) -> bool {
    ALLOWED_GPT_REFRESH_SECONDS.contains(&seconds)
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

/// 将组件配置序列化并写入配置文件。
///
/// # Errors
/// 当目录创建、JSON 序列化或文件写入失败时返回中文错误信息。
fn save_widget_config(path: &Path, config: &WidgetConfig) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("无法创建组件配置目录 {}：{error}", parent.display()))?;
    }
    let content = serde_json::to_string_pretty(config)
        .map_err(|error| format!("无法序列化组件配置：{error}"))?;
    fs::write(path, content)
        .map_err(|error| format!("无法写入组件配置文件 {}：{error}", path.display()))
}

/// 从磁盘加载组件配置，缺失或无效时保存并返回默认配置。
/// 文件读取和默认配置写入失败时会记录中文错误并继续使用默认值。
fn load_widget_config(path: &Path) -> WidgetConfig {
    match fs::read_to_string(path) {
        Ok(content) => match serde_json::from_str::<WidgetConfig>(&content) {
            Ok(config) if is_valid_refresh_seconds(config.gpt_refresh_seconds) => config,
            Ok(_) => {
                eprintln!("组件配置中的 GPT 刷新间隔无效，将恢复默认配置");
                let config = WidgetConfig::default();
                if let Err(error) = save_widget_config(path, &config) {
                    eprintln!("保存默认组件配置失败：{error}");
                }
                config
            }
            Err(error) => {
                eprintln!("组件配置文件无效，将恢复默认配置：{error}");
                let config = WidgetConfig::default();
                if let Err(save_error) = save_widget_config(path, &config) {
                    eprintln!("保存默认组件配置失败：{save_error}");
                }
                config
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let config = WidgetConfig::default();
            if let Err(save_error) = save_widget_config(path, &config) {
                eprintln!("创建默认组件配置失败：{save_error}");
            }
            config
        }
        Err(error) => {
            eprintln!("读取组件配置文件失败，将恢复默认配置：{error}");
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
        taskbar::set_width(&window, width, position)
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

    Ok(SystemUsage {
        cpu_percent,
        memory_percent,
        temperature_celsius,
        disk_percent,
        upload_bytes_per_second: upload_bytes as f64 / elapsed_seconds,
        download_bytes_per_second: download_bytes as f64 / elapsed_seconds,
    })
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
    let proxy_url = match state.config.lock() {
        Ok(config) => config.gpt_proxy_url.trim().to_string(),
        Err(error) => {
            let message = format!("无法读取 GPT 代理配置：{error}");
            append_error_log(&state.log_path, &message);
            return Err(message);
        }
    };
    let result = async {
        let access_token = load_codex_access_token()?;
        let mut client_builder = reqwest::Client::builder().timeout(Duration::from_secs(30));
        if !proxy_url.is_empty() {
            let proxy = reqwest::Proxy::all(&proxy_url)
                .map_err(|_| "GPT 代理地址无效，请检查 widget-config.json 中的 gpt_proxy_url".to_string())?;
            client_builder = client_builder.proxy(proxy);
        }
        let client = client_builder
            .build()
            .map_err(|error| format!("无法创建 Codex 用量请求客户端：{error}"))?;

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
    result
}

/// 将当前配置同步到所有托盘复选菜单项。
fn sync_tray_checks(items: &TrayMenuItems, config: &WidgetConfig) {
    let checks = [
        (&items.position_left, config.position == WidgetPosition::Left, "靠左"),
        (&items.position_right, config.position == WidgetPosition::Right, "靠右"),
        (&items.cpu, config.cpu_visible, "CPU"),
        (&items.memory, config.memory_visible, "内存"),
        (&items.gpt, config.gpt_visible, "GPT"),
        (&items.gpt_remaining, config.gpt_display_mode == GptDisplayMode::Remaining, "GPT 余量"),
        (&items.gpt_used, config.gpt_display_mode == GptDisplayMode::Used, "GPT 用量"),
        (&items.temperature, config.temperature_visible, "温度"),
        (&items.disk, config.disk_visible, "磁盘"),
        (&items.upload, config.upload_visible, "上行流量"),
        (&items.download, config.download_visible, "下行流量"),
        (&items.refresh_30, config.gpt_refresh_seconds == 30, "30秒"),
        (&items.refresh_60, config.gpt_refresh_seconds == 60, "1分钟"),
        (&items.refresh_180, config.gpt_refresh_seconds == 180, "3分钟"),
        (&items.refresh_300, config.gpt_refresh_seconds == 300, "5分钟"),
        (&items.refresh_600, config.gpt_refresh_seconds == 600, "10分钟"),
    ];
    for (item, checked, label) in checks {
        if let Err(error) = item.set_checked(checked) {
            eprintln!("设置托盘菜单“{label}”勾选状态失败：{error}");
        }
    }
}

/// 处理托盘菜单操作并持久化、广播更新后的配置。
fn handle_tray_menu_event(app: &AppHandle, items: &TrayMenuItems, event: MenuEvent) {
    let id = event.id().as_ref();
    if id == "quit" {
        app.exit(0);
        return;
    }

    let state = app.state::<WidgetState>();
    if id == "open_log" {
        if let Err(error) = open_log_file(&state.log_path) {
            append_error_log(&state.log_path, &error);
            eprintln!("{error}");
        }
        return;
    }
    let config = {
        let mut config = match state.config.lock() {
            Ok(config) => config,
            Err(error) => {
                eprintln!("托盘操作无法锁定组件配置状态：{error}");
                return;
            }
        };
        match id {
            "position_left" => config.position = WidgetPosition::Left,
            "position_right" => config.position = WidgetPosition::Right,
            "cpu_visible" => config.cpu_visible = !config.cpu_visible,
            "memory_visible" => config.memory_visible = !config.memory_visible,
            "gpt_visible" => config.gpt_visible = !config.gpt_visible,
            "gpt_display_remaining" => config.gpt_display_mode = GptDisplayMode::Remaining,
            "gpt_display_used" => config.gpt_display_mode = GptDisplayMode::Used,
            "temperature_visible" => config.temperature_visible = !config.temperature_visible,
            "disk_visible" => config.disk_visible = !config.disk_visible,
            "upload_visible" => config.upload_visible = !config.upload_visible,
            "download_visible" => config.download_visible = !config.download_visible,
            "refresh_30" => config.gpt_refresh_seconds = 30,
            "refresh_60" => config.gpt_refresh_seconds = 60,
            "refresh_180" => config.gpt_refresh_seconds = 180,
            "refresh_300" => config.gpt_refresh_seconds = 300,
            "refresh_600" => config.gpt_refresh_seconds = 600,
            _ => return,
        }
        config.clone()
    };

    sync_tray_checks(items, &config);
    if let Err(error) = save_widget_config(&state.config_path, &config) {
        eprintln!("托盘操作保存组件配置失败：{error}");
    }
    if let Err(error) = app.emit("widget-config-changed", &config) {
        eprintln!("托盘操作发送组件配置变更事件失败：{error}");
    }
}

/// 创建仅通过右键打开的系统托盘配置菜单。
///
/// # Errors
/// 当菜单项、菜单、图标或托盘创建失败时返回中文错误信息。
fn setup_tray(app: &AppHandle, config: &WidgetConfig) -> Result<(), String> {
    let position_left = CheckMenuItem::with_id(
        app,
        "position_left",
        "靠左",
        true,
        config.position == WidgetPosition::Left,
        None::<&str>,
    )
    .map_err(|error| format!("无法创建靠左位置菜单：{error}"))?;
    let position_right = CheckMenuItem::with_id(
        app,
        "position_right",
        "靠右",
        true,
        config.position == WidgetPosition::Right,
        None::<&str>,
    )
    .map_err(|error| format!("无法创建靠右位置菜单：{error}"))?;
    let position_menu = Submenu::with_items(
        app,
        "组件位置",
        true,
        &[&position_left, &position_right],
    )
    .map_err(|error| format!("无法创建组件位置子菜单：{error}"))?;
    let cpu = CheckMenuItem::with_id(app, "cpu_visible", "CPU", true, config.cpu_visible, None::<&str>)
        .map_err(|error| format!("无法创建 CPU 托盘菜单：{error}"))?;
    let memory = CheckMenuItem::with_id(app, "memory_visible", "内存", true, config.memory_visible, None::<&str>)
        .map_err(|error| format!("无法创建内存托盘菜单：{error}"))?;
    let gpt = CheckMenuItem::with_id(app, "gpt_visible", "GPT", true, config.gpt_visible, None::<&str>)
        .map_err(|error| format!("无法创建 GPT 托盘菜单：{error}"))?;
    let gpt_remaining = CheckMenuItem::with_id(
        app,
        "gpt_display_remaining",
        "显示余量",
        true,
        config.gpt_display_mode == GptDisplayMode::Remaining,
        None::<&str>,
    )
    .map_err(|error| format!("无法创建 GPT 余量显示菜单：{error}"))?;
    let gpt_used = CheckMenuItem::with_id(
        app,
        "gpt_display_used",
        "显示用量",
        true,
        config.gpt_display_mode == GptDisplayMode::Used,
        None::<&str>,
    )
    .map_err(|error| format!("无法创建 GPT 用量显示菜单：{error}"))?;
    let gpt_display_menu = Submenu::with_items(
        app,
        "GPT 显示方式",
        true,
        &[&gpt_remaining, &gpt_used],
    )
    .map_err(|error| format!("无法创建 GPT 显示方式子菜单：{error}"))?;
    let temperature = CheckMenuItem::with_id(app, "temperature_visible", "温度", true, config.temperature_visible, None::<&str>)
        .map_err(|error| format!("无法创建温度托盘菜单：{error}"))?;
    let disk = CheckMenuItem::with_id(app, "disk_visible", "磁盘", true, config.disk_visible, None::<&str>)
        .map_err(|error| format!("无法创建磁盘托盘菜单：{error}"))?;
    let upload = CheckMenuItem::with_id(app, "upload_visible", "上行流量", true, config.upload_visible, None::<&str>)
        .map_err(|error| format!("无法创建上行流量托盘菜单：{error}"))?;
    let download = CheckMenuItem::with_id(app, "download_visible", "下行流量", true, config.download_visible, None::<&str>)
        .map_err(|error| format!("无法创建下行流量托盘菜单：{error}"))?;
    let refresh_30 = CheckMenuItem::with_id(app, "refresh_30", "30秒", true, config.gpt_refresh_seconds == 30, None::<&str>)
        .map_err(|error| format!("无法创建 30 秒刷新菜单：{error}"))?;
    let refresh_60 = CheckMenuItem::with_id(app, "refresh_60", "1分钟", true, config.gpt_refresh_seconds == 60, None::<&str>)
        .map_err(|error| format!("无法创建 1 分钟刷新菜单：{error}"))?;
    let refresh_180 = CheckMenuItem::with_id(app, "refresh_180", "3分钟", true, config.gpt_refresh_seconds == 180, None::<&str>)
        .map_err(|error| format!("无法创建 3 分钟刷新菜单：{error}"))?;
    let refresh_300 = CheckMenuItem::with_id(app, "refresh_300", "5分钟", true, config.gpt_refresh_seconds == 300, None::<&str>)
        .map_err(|error| format!("无法创建 5 分钟刷新菜单：{error}"))?;
    let refresh_600 = CheckMenuItem::with_id(app, "refresh_600", "10分钟", true, config.gpt_refresh_seconds == 600, None::<&str>)
        .map_err(|error| format!("无法创建 10 分钟刷新菜单：{error}"))?;
    let refresh_menu = Submenu::with_items(
        app,
        "GPT 刷新间隔",
        true,
        &[&refresh_30, &refresh_60, &refresh_180, &refresh_300, &refresh_600],
    )
    .map_err(|error| format!("无法创建 GPT 刷新子菜单：{error}"))?;
    let separator = PredefinedMenuItem::separator(app)
        .map_err(|error| format!("无法创建托盘菜单分隔线：{error}"))?;
    let open_log = MenuItem::with_id(app, "open_log", "打开日志", true, None::<&str>)
        .map_err(|error| format!("无法创建打开日志菜单：{error}"))?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)
        .map_err(|error| format!("无法创建退出菜单：{error}"))?;
    let menu = Menu::with_items(
        app,
        &[&cpu, &memory, &gpt, &temperature, &disk, &upload, &download, &position_menu, &gpt_display_menu, &refresh_menu, &separator, &open_log, &quit],
    )
    .map_err(|error| format!("无法创建托盘菜单：{error}"))?;
    let items = TrayMenuItems {
        position_left,
        position_right,
        cpu,
        memory,
        gpt,
        gpt_remaining,
        gpt_used,
        temperature,
        disk,
        upload,
        download,
        refresh_30,
        refresh_60,
        refresh_180,
        refresh_300,
        refresh_600,
    };
    let event_items = items.clone();
    let icon = app
        .default_window_icon()
        .cloned()
        .ok_or_else(|| "无法获取默认窗口图标".to_string())?;

    TrayIconBuilder::new()
        .icon(icon)
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(move |app, event| handle_tray_menu_event(app, &event_items, event))
        .build(app)
        .map_err(|error| format!("无法创建系统托盘图标：{error}"))?;
    Ok(())
}

/// 获取配置和日志使用的运行目录。
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

/// 初始化配置、日志、持久采样器和系统托盘。
///
/// # Errors
/// 当运行目录、日志文件、配置文件或托盘创建失败时返回错误。
fn setup_app(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let data_dir = runtime_data_dir().map_err(std::io::Error::other)?;
    let config_path = data_dir.join(CONFIG_FILE_NAME);
    let log_path = data_dir.join(LOG_FILE_NAME);
    fs::write(&log_path, "").map_err(|error| {
        std::io::Error::other(format!(
            "无法在每次启动时重建组件日志文件 {}：{error}",
            log_path.display()
        ))
    })?;
    let config = load_widget_config(&config_path);
    app.manage(WidgetState {
        config: Mutex::new(config.clone()),
        config_path,
        log_path,
        system: Mutex::new(System::new_all()),
        components: Mutex::new(Components::new_with_refreshed_list()),
        disks: Mutex::new(Disks::new_with_refreshed_list()),
        networks: Mutex::new(Networks::new_with_refreshed_list()),
        network_instant: Mutex::new(Instant::now()),
    });
    setup_tray(app.handle(), &config).map_err(std::io::Error::other)?;
    Ok(())
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
            if let Err(error) = taskbar::attach(&window, position) {
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

/// 创建 Tauri 应用，注册系统监控命令、配置状态和托盘菜单。
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(setup_app)
        .invoke_handler(tauri::generate_handler![
            set_widget_width,
            get_widget_config,
            get_system_usage,
            open_task_manager,
            get_codex_usage
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(handle_run_event);
}
