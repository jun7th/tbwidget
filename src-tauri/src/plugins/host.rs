use std::{collections::BTreeMap, fs, path::PathBuf, time::Duration};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{AppHandle, Emitter};

use crate::{append_log, WidgetState};
use super::{
    ensure_plugin_system_ready, load_registered_manifests, load_runtime_config,
    plugin_config_path_from_runtime, read_plugin_config, write_plugin_config,
};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginHttpRequest {
    url: String,
    #[serde(default = "default_http_method")]
    method: String,
    #[serde(default)]
    headers: BTreeMap<String, String>,
    body: Option<String>,
    timeout_ms: Option<u64>,
    proxy_url: Option<String>,
    response_type: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PluginStorageChanged {
    plugin_id: String,
    key: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginHttpResponse {
    status: u16,
    ok: bool,
    headers: BTreeMap<String, String>,
    body: Option<String>,
    bytes: Option<Vec<u8>>,
}

fn default_http_method() -> String {
    "GET".to_string()
}

fn validate_plugin_permission(state: &WidgetState, plugin_id: &str, permission: &str) -> Result<(), String> {
    ensure_plugin_system_ready(state)?;
    let manifests = load_registered_manifests(state)?;
    let manifest = manifests
        .iter()
        .find_map(|(_, manifest)| (manifest.id == plugin_id).then_some(manifest))
        .ok_or_else(|| format!("找不到插件：{plugin_id}"))?;
    if !manifest.permissions.iter().any(|item| item == permission) {
        return Err(format!("插件 {plugin_id} 没有 {permission} 权限"));
    }
    let runtime = load_runtime_config(&state.config_path)?;
    if !runtime
        .plugins
        .get(plugin_id)
        .map(|entry| entry.enabled)
        .unwrap_or(false)
    {
        return Err(format!("插件 {plugin_id} 未启用"));
    }
    Ok(())
}

fn expand_user_path(path: &str) -> Result<PathBuf, String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err("文件路径不能为空".to_string());
    }
    if trimmed == "~" || trimmed.starts_with("~/") || trimmed.starts_with("~\\") {
        let home = std::env::var_os("USERPROFILE")
            .or_else(|| std::env::var_os("HOME"))
            .map(PathBuf::from)
            .ok_or_else(|| "无法确定当前用户目录".to_string())?;
        if trimmed == "~" {
            return Ok(home);
        }
        return Ok(home.join(&trimmed[2..]));
    }
    Ok(PathBuf::from(trimmed))
}

#[tauri::command]
pub fn plugin_fs_read_text(
    state: tauri::State<'_, WidgetState>,
    plugin_id: String,
    path: String,
) -> Result<String, String> {
    validate_plugin_permission(state.inner(), &plugin_id, "fs.read")?;
    let path = expand_user_path(&path)?;
    fs::read_to_string(&path)
        .map_err(|error| format!("无法读取文件 {}：{error}", path.display()))
}

#[tauri::command]
pub fn plugin_fs_read_bytes(
    state: tauri::State<'_, WidgetState>,
    plugin_id: String,
    path: String,
) -> Result<Vec<u8>, String> {
    validate_plugin_permission(state.inner(), &plugin_id, "fs.read")?;
    let path = expand_user_path(&path)?;
    fs::read(&path).map_err(|error| format!("无法读取文件 {}：{error}", path.display()))
}

#[tauri::command]
pub async fn plugin_http_request(
    state: tauri::State<'_, WidgetState>,
    plugin_id: String,
    request: PluginHttpRequest,
) -> Result<PluginHttpResponse, String> {
    validate_plugin_permission(state.inner(), &plugin_id, "http.request")?;

    let url = reqwest::Url::parse(request.url.trim())
        .map_err(|error| format!("HTTP URL 无效：{error}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("HTTP 请求只允许 http:// 或 https://".to_string());
    }

    let method = reqwest::Method::from_bytes(request.method.trim().as_bytes())
        .map_err(|error| format!("HTTP method 无效：{error}"))?;
    let timeout = request.timeout_ms.unwrap_or(15_000).clamp(100, 120_000);
    let mut client_builder = reqwest::Client::builder().timeout(Duration::from_millis(timeout));
    if let Some(proxy_url) = request.proxy_url.as_deref().map(str::trim).filter(|value| !value.is_empty()) {
        let proxy = reqwest::Proxy::all(proxy_url)
            .map_err(|error| format!("HTTP 代理地址无效：{error}"))?;
        client_builder = client_builder.proxy(proxy);
    }
    let client = client_builder
        .build()
        .map_err(|error| format!("无法创建 HTTP 客户端：{error}"))?;

    let mut builder = client.request(method, url);
    for (name, value) in request.headers {
        let name = reqwest::header::HeaderName::from_bytes(name.as_bytes())
            .map_err(|error| format!("HTTP header 名称无效：{error}"))?;
        let value = reqwest::header::HeaderValue::from_str(&value)
            .map_err(|error| format!("HTTP header 值无效：{error}"))?;
        builder = builder.header(name, value);
    }
    if let Some(body) = request.body {
        builder = builder.body(body);
    }

    let response = builder.send().await.map_err(|error| format!("HTTP 请求失败：{error}"))?;
    let status = response.status();
    if response.content_length().is_some_and(|length| length > 16 * 1024 * 1024) {
        return Err("HTTP 响应超过 16 MiB 限制".to_string());
    }
    let headers = response
        .headers()
        .iter()
        .map(|(name, value)| (name.as_str().to_string(), value.to_str().unwrap_or_default().to_string()))
        .collect::<BTreeMap<_, _>>();
    let bytes = response.bytes().await.map_err(|error| format!("读取 HTTP 响应失败：{error}"))?;
    if bytes.len() > 16 * 1024 * 1024 {
        return Err("HTTP 响应超过 16 MiB 限制".to_string());
    }
    let wants_bytes = request.response_type.as_deref() == Some("bytes");
    Ok(PluginHttpResponse {
        status: status.as_u16(),
        ok: status.is_success(),
        headers,
        body: (!wants_bytes).then(|| String::from_utf8_lossy(&bytes).into_owned()),
        bytes: wants_bytes.then(|| bytes.to_vec()),
    })
}

fn validate_storage_plugin(state: &WidgetState, plugin_id: &str) -> Result<(), String> {
    validate_plugin_permission(state, plugin_id, "storage")
}

/// 从插件自己的 save/config/plugin/<目录>/config.json 读取数据。
#[tauri::command]
pub fn plugin_storage_get(
    state: tauri::State<'_, WidgetState>,
    plugin_id: String,
    key: String,
) -> Result<Option<Value>, String> {
    validate_storage_plugin(state.inner(), &plugin_id)?;
    let _guard = state
        .config_file_lock
        .lock()
        .map_err(|error| format!("无法锁定配置文件：{error}"))?;
    let runtime = load_runtime_config(&state.config_path)?;
    let path = plugin_config_path_from_runtime(state.inner(), &runtime, &plugin_id)?;
    let storage = read_plugin_config(&path)?;
    Ok(storage.get(&key).cloned())
}

/// 将插件设置或缓存写入插件自己的 config.json。
#[tauri::command]
pub fn plugin_storage_set(
    app: AppHandle,
    state: tauri::State<'_, WidgetState>,
    plugin_id: String,
    key: String,
    value: Value,
) -> Result<(), String> {
    validate_storage_plugin(state.inner(), &plugin_id)?;
    let path = {
        let _guard = state
            .config_file_lock
            .lock()
            .map_err(|error| format!("无法锁定配置文件：{error}"))?;
        let runtime = load_runtime_config(&state.config_path)?;
        let path = plugin_config_path_from_runtime(state.inner(), &runtime, &plugin_id)?;
        let mut storage = match read_plugin_config(&path) {
            Ok(storage) => storage,
            Err(error) => {
                append_log(
                    &state.log_path,
                    "ERROR",
                    "plugin-config",
                    &format!("插件 {} 配置读取失败 key={}：{error}", plugin_id, key),
                );
                return Err(error);
            }
        };
        storage.insert(key.clone(), value);
        if let Err(error) = write_plugin_config(&path, &storage) {
            append_log(
                &state.log_path,
                "ERROR",
                "plugin-config",
                &format!("插件 {} 配置写入失败 key={}：{error}", plugin_id, key),
            );
            return Err(error);
        }
        path
    };
    let log_level = if key == "usageCache" { "DEBUG" } else { "ACTION" };
    append_log(
        &state.log_path,
        log_level,
        "plugin-config",
        &format!(
            "插件 {} {}成功 key={} file={}",
            plugin_id,
            if key == "settings" { "设置修改" } else if key == "usageCache" { "用量缓存写入" } else { "配置修改" },
            key,
            path.display()
        ),
    );
    app.emit(
        "plugin-storage-changed",
        PluginStorageChanged {
            plugin_id,
            key,
        },
    )
    .map_err(|error| format!("发送插件存储变更事件失败：{error}"))?;
    Ok(())
}

/// 删除插件自己 config.json 中的单个键。
#[tauri::command]
pub fn plugin_storage_remove(
    state: tauri::State<'_, WidgetState>,
    plugin_id: String,
    key: String,
) -> Result<(), String> {
    validate_storage_plugin(state.inner(), &plugin_id)?;
    let _guard = state
        .config_file_lock
        .lock()
        .map_err(|error| format!("无法锁定配置文件：{error}"))?;
    let runtime = load_runtime_config(&state.config_path)?;
    let path = plugin_config_path_from_runtime(state.inner(), &runtime, &plugin_id)?;
    let mut storage = read_plugin_config(&path)?;
    storage.remove(&key);
    write_plugin_config(&path, &storage)?;
    append_log(
        &state.log_path,
        "ACTION",
        "plugin-config",
        &format!("插件 {} 配置键已删除 key={}", plugin_id, key),
    );
    Ok(())
}

/// 清空一个插件自己的 config.json，不影响主配置和其他插件。
#[tauri::command]
pub fn plugin_storage_clear(
    state: tauri::State<'_, WidgetState>,
    plugin_id: String,
) -> Result<(), String> {
    validate_storage_plugin(state.inner(), &plugin_id)?;
    let _guard = state
        .config_file_lock
        .lock()
        .map_err(|error| format!("无法锁定配置文件：{error}"))?;
    let runtime = load_runtime_config(&state.config_path)?;
    let path = plugin_config_path_from_runtime(state.inner(), &runtime, &plugin_id)?;
    write_plugin_config(&path, &BTreeMap::new())?;
    append_log(
        &state.log_path,
        "ACTION",
        "plugin-config",
        &format!("插件 {} 配置已清空 file={}", plugin_id, path.display()),
    );
    Ok(())
}

/// 系统资源监视是内置特殊能力：仅 system-monitor 插件可调用。
#[tauri::command]
pub async fn plugin_system_metrics(
    state: tauri::State<'_, WidgetState>,
    plugin_id: String,
) -> Result<crate::system_monitor::SystemMetrics, String> {
    if plugin_id != "system-monitor" {
        return Err("system.metrics 仅提供给内置系统资源监视插件".to_string());
    }
    validate_plugin_permission(state.inner(), &plugin_id, "system.metrics")?;

    // sysinfo 首次初始化会枚举 CPU/磁盘/网络/传感器。放到 blocking 线程并懒加载，
    // 避免 setup_app 和 WebView IPC 主路径出现肉眼可见的启动卡顿。
    let sampler = std::sync::Arc::clone(&state.system_monitor);
    tauri::async_runtime::spawn_blocking(move || {
        let mut guard = sampler
            .lock()
            .map_err(|error| format!("无法锁定系统资源采样器：{error}"))?;
        let sampler = guard.get_or_insert_with(crate::system_monitor::SystemMonitorSampler::new);
        sampler.sample()
    })
    .await
    .map_err(|error| format!("系统资源采样任务失败：{error}"))?
}
