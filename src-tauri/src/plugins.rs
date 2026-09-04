use std::{
    collections::BTreeMap,
    fs,
    path::{Component, Path, PathBuf},
    time::Duration,
};

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{AppHandle, Emitter, LogicalSize, Manager, PhysicalPosition};

#[cfg(target_os = "windows")]
use windows_sys::Win32::{
    Foundation::{HWND, POINT, RECT},
    Graphics::Gdi::{GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST},
    UI::WindowsAndMessaging::GetWindowRect,
};

use crate::{unix_timestamp, WidgetState};

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct PluginManifest {
    manifest_version: u32,
    id: String,
    name: String,
    #[serde(default)]
    description: String,
    version: String,
    panel: Option<String>,
    popup: Option<String>,
    settings: Option<String>,
    wasm: Option<String>,
    #[serde(default)]
    permissions: Vec<String>,
    #[serde(default)]
    builtin: bool,
}

#[derive(Clone, Default, Deserialize, Serialize)]
struct PluginEntryState {
    enabled: bool,
    order: i32,
}

#[derive(Default, Deserialize, Serialize)]
struct PluginRuntimeConfig {
    #[serde(default)]
    plugins: BTreeMap<String, PluginEntryState>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PluginBundle {
    id: String,
    name: String,
    description: String,
    version: String,
    manifest_version: u32,
    permissions: Vec<String>,
    enabled: bool,
    order: i32,
    panel_html: String,
    panel_js: String,
    popup_html: Option<String>,
    popup_js: Option<String>,
    settings_js: Option<String>,
    wasm_bytes: Option<Vec<u8>>,
    builtin: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PluginHttpRequest {
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
pub(crate) struct PluginHttpResponse {
    status: u16,
    ok: bool,
    headers: BTreeMap<String, String>,
    body: Option<String>,
    bytes: Option<Vec<u8>>,
}

fn default_http_method() -> String {
    "GET".to_string()
}

fn load_runtime_config(path: &Path) -> PluginRuntimeConfig {
    fs::read_to_string(path)
        .ok()
        .and_then(|content| serde_json::from_str(&content).ok())
        .unwrap_or_default()
}

fn save_runtime_config(path: &Path, config: &PluginRuntimeConfig) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("无法创建插件配置目录 {}：{error}", parent.display()))?;
    }
    let content = serde_json::to_string_pretty(config)
        .map_err(|error| format!("无法序列化插件配置：{error}"))?;
    fs::write(path, content)
        .map_err(|error| format!("无法写入插件配置 {}：{error}", path.display()))
}

fn safe_plugin_path(root: &Path, relative: &str) -> Result<PathBuf, String> {
    let relative_path = Path::new(relative);
    if relative_path.is_absolute()
        || relative_path
            .components()
            .any(|component| matches!(component, Component::ParentDir | Component::RootDir | Component::Prefix(_)))
    {
        return Err(format!("插件文件路径不安全：{relative}"));
    }
    Ok(root.join(relative_path))
}

fn read_text(root: &Path, relative: &str) -> Result<String, String> {
    let path = safe_plugin_path(root, relative)?;
    fs::read_to_string(&path)
        .map_err(|error| format!("无法读取插件文件 {}：{error}", path.display()))
}

fn sibling_script(path: &str) -> String {
    let mut script = PathBuf::from(path);
    script.set_extension("js");
    script.to_string_lossy().replace('\\', "/")
}

fn scan_manifests(plugins_dir: &Path) -> Result<Vec<(PathBuf, PluginManifest)>, String> {
    fs::create_dir_all(plugins_dir)
        .map_err(|error| format!("无法创建插件目录 {}：{error}", plugins_dir.display()))?;

    let mut manifests = Vec::new();
    let entries = fs::read_dir(plugins_dir)
        .map_err(|error| format!("无法扫描插件目录 {}：{error}", plugins_dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("无法读取插件目录项：{error}"))?;
        let file_type = entry
            .file_type()
            .map_err(|error| format!("无法读取插件目录项类型：{error}"))?;
        if !file_type.is_dir() {
            continue;
        }
        let root = entry.path();
        let manifest_path = root.join("plugin.json");
        if !manifest_path.is_file() {
            continue;
        }
        let content = fs::read_to_string(&manifest_path)
            .map_err(|error| format!("无法读取 {}：{error}", manifest_path.display()))?;
        let manifest: PluginManifest = serde_json::from_str(&content)
            .map_err(|error| format!("插件清单 {} 无效：{error}", manifest_path.display()))?;
        if manifest.manifest_version != 1 {
            return Err(format!(
                "插件 {} 使用不支持的 manifestVersion={}，当前仅支持 1",
                manifest.id, manifest.manifest_version
            ));
        }
        if manifest.id.trim().is_empty()
            || manifest.id.contains('/')
            || manifest.id.contains('\\')
            || manifest.id == "."
            || manifest.id == ".."
        {
            return Err(format!("插件目录 {} 的 id 无效", root.display()));
        }
        manifests.push((root, manifest));
    }
    manifests.sort_by(|left, right| left.1.id.cmp(&right.1.id));
    Ok(manifests)
}

#[tauri::command]
pub(crate) fn list_plugins(
    state: tauri::State<'_, WidgetState>,
) -> Result<Vec<PluginBundle>, String> {
    let manifests = scan_manifests(&state.plugins_dir)?;
    let mut runtime = load_runtime_config(&state.plugin_config_path);
    let mut changed = false;
    let mut next_order = runtime
        .plugins
        .values()
        .map(|entry| entry.order)
        .max()
        .unwrap_or(-1)
        + 1;

    for (_, manifest) in &manifests {
        if !runtime.plugins.contains_key(&manifest.id) {
            runtime.plugins.insert(
                manifest.id.clone(),
                PluginEntryState {
                    enabled: false,
                    order: next_order,
                },
            );
            next_order += 1;
            changed = true;
        }
    }
    if changed {
        save_runtime_config(&state.plugin_config_path, &runtime)?;
    }

    let mut bundles = Vec::with_capacity(manifests.len());
    for (root, manifest) in manifests {
        let entry = runtime.plugins.get(&manifest.id).cloned().unwrap_or_default();
        let (panel_html, panel_js) = if let Some(panel_path) = manifest.panel.as_deref() {
            let panel_file = safe_plugin_path(&root, panel_path)?;
            if panel_file.is_file() {
                let html = fs::read_to_string(&panel_file)
                    .map_err(|error| format!("无法读取插件文件 {}：{error}", panel_file.display()))?;
                let script_path = sibling_script(panel_path);
                let script_file = safe_plugin_path(&root, &script_path)?;
                let js = if script_file.is_file() {
                    fs::read_to_string(&script_file)
                        .map_err(|error| format!("无法读取插件文件 {}：{error}", script_file.display()))?
                } else {
                    String::new()
                };
                (html, js)
            } else {
                (String::new(), String::new())
            }
        } else {
            (String::new(), String::new())
        };
        let popup_html = manifest
            .popup
            .as_deref()
            .map(|path| read_text(&root, path))
            .transpose()?;
        let popup_js = manifest
            .popup
            .as_deref()
            .map(sibling_script)
            .map(|path| read_text(&root, &path))
            .transpose()?;
        let settings_js = manifest
            .settings
            .as_deref()
            .map(|path| read_text(&root, path))
            .transpose()?;
        let wasm_bytes = manifest
            .wasm
            .as_deref()
            .map(|path| safe_plugin_path(&root, path).and_then(|path| {
                fs::read(&path)
                    .map_err(|error| format!("无法读取插件 WASM {}：{error}", path.display()))
            }))
            .transpose()?;

        bundles.push(PluginBundle {
            id: manifest.id,
            name: manifest.name,
            description: manifest.description,
            version: manifest.version,
            manifest_version: manifest.manifest_version,
            permissions: manifest.permissions,
            enabled: entry.enabled,
            order: entry.order,
            panel_html,
            panel_js,
            popup_html,
            popup_js,
            settings_js,
            wasm_bytes,
            builtin: manifest.builtin,
        });
    }
    bundles.sort_by_key(|plugin| plugin.order);
    Ok(bundles)
}


#[tauri::command]
pub(crate) fn refresh_plugins(
    app: AppHandle,
    state: tauri::State<'_, WidgetState>,
) -> Result<Vec<PluginBundle>, String> {
    let bundles = list_plugins(state)?;
    app.emit("plugins-changed", ())
        .map_err(|error| format!("发送插件刷新事件失败：{error}"))?;
    Ok(bundles)
}

#[tauri::command]
pub(crate) fn set_plugin_enabled(
    app: AppHandle,
    state: tauri::State<'_, WidgetState>,
    plugin_id: String,
    enabled: bool,
) -> Result<(), String> {
    let manifests = scan_manifests(&state.plugins_dir)?;
    if !manifests.iter().any(|(_, manifest)| manifest.id == plugin_id) {
        return Err(format!("找不到插件：{plugin_id}"));
    }

    let mut runtime = load_runtime_config(&state.plugin_config_path);
    let fallback_order = runtime.plugins.len() as i32;
    let entry = runtime
        .plugins
        .entry(plugin_id.clone())
        .or_insert(PluginEntryState {
            enabled: false,
            order: fallback_order,
        });
    entry.enabled = enabled;
    save_runtime_config(&state.plugin_config_path, &runtime)?;

    app.emit("plugins-changed", ())
        .map_err(|error| format!("发送插件变更事件失败：{error}"))?;
    Ok(())
}

#[tauri::command]
pub(crate) fn set_plugin_order(
    app: AppHandle,
    state: tauri::State<'_, WidgetState>,
    plugin_ids: Vec<String>,
) -> Result<(), String> {
    let manifests = scan_manifests(&state.plugins_dir)?;
    let known = manifests
        .iter()
        .map(|(_, manifest)| manifest.id.clone())
        .collect::<std::collections::BTreeSet<_>>();
    if plugin_ids.iter().any(|id| !known.contains(id)) {
        return Err("插件排序中包含未知插件".to_string());
    }

    let requested = plugin_ids.iter().cloned().collect::<std::collections::BTreeSet<_>>();
    let mut runtime = load_runtime_config(&state.plugin_config_path);
    let mut remaining = runtime
        .plugins
        .iter()
        .filter(|(id, _)| !requested.contains(*id))
        .map(|(id, entry)| (id.clone(), entry.order))
        .collect::<Vec<_>>();
    remaining.sort_by_key(|(_, order)| *order);

    let mut order = 0_i32;
    for plugin_id in plugin_ids {
        let fallback_order = runtime.plugins.len() as i32;
        let entry = runtime.plugins.entry(plugin_id).or_insert(PluginEntryState { enabled: false, order: fallback_order });
        entry.order = order;
        order += 1;
    }
    for (plugin_id, _) in remaining {
        if let Some(entry) = runtime.plugins.get_mut(&plugin_id) {
            entry.order = order;
            order += 1;
        }
    }

    save_runtime_config(&state.plugin_config_path, &runtime)?;
    app.emit("plugins-changed", ())
        .map_err(|error| format!("发送插件排序变更事件失败：{error}"))?;
    Ok(())
}

const PLUGIN_POPUP_INITIAL_WIDTH: f64 = 160.0;
const PLUGIN_POPUP_INITIAL_HEIGHT: f64 = 88.0;
const PLUGIN_POPUP_GAP: f64 = 8.0;

fn validate_popup_plugin(state: &WidgetState, plugin_id: &str) -> Result<(), String> {
    let manifests = scan_manifests(&state.plugins_dir)?;
    let manifest = manifests
        .iter()
        .find_map(|(_, manifest)| (manifest.id == plugin_id).then_some(manifest))
        .ok_or_else(|| format!("找不到插件：{plugin_id}"))?;
    if manifest.popup.is_none() {
        return Err(format!("插件 {plugin_id} 没有 popup"));
    }
    let runtime = load_runtime_config(&state.plugin_config_path);
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

fn validate_settings_plugin(state: &WidgetState, plugin_id: &str) -> Result<(), String> {
    let manifests = scan_manifests(&state.plugins_dir)?;
    let manifest = manifests
        .iter()
        .find_map(|(_, manifest)| (manifest.id == plugin_id).then_some(manifest))
        .ok_or_else(|| format!("找不到插件：{plugin_id}"))?;
    if manifest.settings.is_none() {
        return Err(format!("插件 {plugin_id} 没有 settings"));
    }
    let runtime = load_runtime_config(&state.plugin_config_path);
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

fn position_plugin_popup(
    popup: &tauri::WebviewWindow,
    anchor_screen_x: i32,
    taskbar_top: i32,
    width: f64,
    height: f64,
) -> Result<(), String> {
    let scale = popup
        .scale_factor()
        .map_err(|error| format!("无法读取插件弹窗缩放比例：{error}"))?;
    let mut width_px = (width * scale).round().max(1.0) as i32;
    let mut height_px = (height * scale).round().max(1.0) as i32;
    let gap_px = (PLUGIN_POPUP_GAP * scale).round() as i32;

    let mut x = anchor_screen_x - width_px / 2;
    let mut y = taskbar_top - height_px - gap_px;

    #[cfg(target_os = "windows")]
    unsafe {
        let monitor = MonitorFromPoint(
            POINT {
                x: anchor_screen_x,
                y: taskbar_top.saturating_sub(1),
            },
            MONITOR_DEFAULTTONEAREST,
        );
        if !monitor.is_null() {
            let mut info: MONITORINFO = std::mem::zeroed();
            info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
            if GetMonitorInfoW(monitor, &mut info) != 0 {
                let work = info.rcWork;
                let margin = gap_px.max(4);
                let available_width = (work.right - work.left - margin * 2).max(1);
                let available_height = (work.bottom - work.top - margin * 2).max(1);
                width_px = width_px.min(available_width);
                height_px = height_px.min(available_height);
                x = anchor_screen_x - width_px / 2;
                y = taskbar_top - height_px - gap_px;
                let min_x = work.left.saturating_add(margin);
                let max_x = work.right.saturating_sub(width_px).saturating_sub(margin);
                x = if max_x >= min_x { x.clamp(min_x, max_x) } else { work.left };

                let min_y = work.top.saturating_add(margin);
                let max_y = work.bottom.saturating_sub(height_px).saturating_sub(margin);
                y = if max_y >= min_y { y.clamp(min_y, max_y) } else { work.top };
            }
        }
    }

    let logical_width = width_px as f64 / scale;
    let logical_height = height_px as f64 / scale;
    popup
        .set_size(LogicalSize::new(logical_width, logical_height))
        .map_err(|error| format!("无法调整插件弹窗尺寸：{error}"))?;
    popup
        .set_position(PhysicalPosition::new(x, y))
        .map_err(|error| format!("无法定位插件弹窗：{error}"))?;
    Ok(())
}

#[tauri::command]
pub(crate) fn show_plugin_popup(
    app: AppHandle,
    state: tauri::State<'_, WidgetState>,
    plugin_id: String,
    anchor_x: f64,
    anchor_width: f64,
) -> Result<(), String> {
    validate_popup_plugin(state.inner(), &plugin_id)?;
    let popup = app
        .get_webview_window("plugin-popup")
        .ok_or_else(|| "找不到 plugin-popup 窗口".to_string())?;
    let main = app
        .get_webview_window("main")
        .ok_or_else(|| "找不到 main 窗口".to_string())?;

    #[cfg(target_os = "windows")]
    let (anchor_screen_x, taskbar_top) = {
        let native = main
            .hwnd()
            .map_err(|error| format!("无法读取主组件 HWND：{error}"))?;
        let hwnd = native.0 as HWND;
        let mut rect: RECT = unsafe { std::mem::zeroed() };
        if unsafe { GetWindowRect(hwnd, &mut rect) } == 0 {
            return Err("无法读取主组件屏幕位置".to_string());
        }
        let scale = main
            .scale_factor()
            .map_err(|error| format!("无法读取主组件缩放比例：{error}"))?;
        let center = anchor_x + anchor_width / 2.0;
        (rect.left + (center * scale).round() as i32, rect.top)
    };

    #[cfg(not(target_os = "windows"))]
    let (anchor_screen_x, taskbar_top) = {
        let position = main
            .outer_position()
            .map_err(|error| format!("无法读取主组件位置：{error}"))?;
        let scale = main
            .scale_factor()
            .map_err(|error| format!("无法读取主组件缩放比例：{error}"))?;
        (
            position.x + ((anchor_x + anchor_width / 2.0) * scale).round() as i32,
            position.y,
        )
    };

    {
        let mut active = state
            .plugin_popup
            .lock()
            .map_err(|error| format!("无法锁定插件弹窗状态：{error}"))?;
        *active = Some(crate::PluginPopupAnchor {
            plugin_id: plugin_id.clone(),
            anchor_screen_x,
            taskbar_top,
        });
    }

    let (initial_width, initial_height) = state
        .plugin_popup_sizes
        .lock()
        .ok()
        .and_then(|sizes| sizes.get(&plugin_id).copied())
        .unwrap_or((PLUGIN_POPUP_INITIAL_WIDTH, PLUGIN_POPUP_INITIAL_HEIGHT));

    position_plugin_popup(
        &popup,
        anchor_screen_x,
        taskbar_top,
        initial_width,
        initial_height,
    )?;
    popup
        .emit("plugin-popup-changed", plugin_id)
        .map_err(|error| format!("无法通知插件弹窗切换：{error}"))?;
    popup
        .show()
        .map_err(|error| format!("无法显示插件弹窗：{error}"))?;
    popup
        .set_focus()
        .map_err(|error| format!("无法聚焦插件弹窗：{error}"))?;
    Ok(())
}

#[tauri::command]
pub(crate) fn resize_plugin_popup(
    app: AppHandle,
    state: tauri::State<'_, WidgetState>,
    width: f64,
    height: f64,
) -> Result<(), String> {
    let active = state
        .plugin_popup
        .lock()
        .map_err(|error| format!("无法锁定插件弹窗状态：{error}"))?
        .clone()
        .ok_or_else(|| "当前没有活动插件弹窗".to_string())?;
    let popup = app
        .get_webview_window("plugin-popup")
        .ok_or_else(|| "找不到 plugin-popup 窗口".to_string())?;
    let width = width.round().clamp(1.0, 4096.0);
    let height = height.round().clamp(1.0, 4096.0);
    position_plugin_popup(
        &popup,
        active.anchor_screen_x,
        active.taskbar_top,
        width,
        height,
    )?;
    if let Ok(mut sizes) = state.plugin_popup_sizes.lock() {
        sizes.insert(active.plugin_id, (width, height));
    }
    Ok(())
}

#[tauri::command]
pub(crate) fn hide_plugin_popup(
    app: AppHandle,
    state: tauri::State<'_, WidgetState>,
) -> Result<(), String> {
    if let Ok(mut active) = state.plugin_popup.lock() {
        *active = None;
    }
    if let Some(popup) = app.get_webview_window("plugin-popup") {
        popup
            .hide()
            .map_err(|error| format!("无法隐藏插件弹窗：{error}"))?;
    }
    Ok(())
}

#[tauri::command]
pub(crate) fn get_active_plugin_popup_id(
    state: tauri::State<'_, WidgetState>,
) -> Result<Option<String>, String> {
    state
        .plugin_popup
        .lock()
        .map(|active| active.as_ref().map(|item| item.plugin_id.clone()))
        .map_err(|error| format!("无法读取插件弹窗状态：{error}"))
}

#[tauri::command]
pub(crate) fn show_plugin_settings_window(
    app: AppHandle,
    state: tauri::State<'_, WidgetState>,
    plugin_id: String,
) -> Result<(), String> {
    validate_settings_plugin(state.inner(), &plugin_id)?;
    {
        let mut active = state
            .plugin_settings
            .lock()
            .map_err(|error| format!("无法锁定插件设置窗口状态：{error}"))?;
        *active = Some(plugin_id.clone());
    }

    let window = app
        .get_webview_window("plugin-settings")
        .ok_or_else(|| "找不到 plugin-settings 窗口".to_string())?;
    window
        .emit("plugin-settings-changed", plugin_id)
        .map_err(|error| format!("无法通知插件设置窗口切换：{error}"))?;
    let _ = window.center();
    window
        .show()
        .map_err(|error| format!("无法显示插件设置窗口：{error}"))?;
    window
        .set_focus()
        .map_err(|error| format!("无法聚焦插件设置窗口：{error}"))?;
    Ok(())
}

#[tauri::command]
pub(crate) fn resize_plugin_settings_window(
    app: AppHandle,
    width: f64,
    height: f64,
) -> Result<(), String> {
    let window = app
        .get_webview_window("plugin-settings")
        .ok_or_else(|| "找不到 plugin-settings 窗口".to_string())?;
    let width = width.round().clamp(400.0, 720.0);
    let height = height.round().clamp(240.0, 760.0);
    window
        .set_size(LogicalSize::new(width, height))
        .map_err(|error| format!("无法调整插件设置窗口尺寸：{error}"))?;
    let _ = window.center();
    Ok(())
}

#[tauri::command]
pub(crate) fn hide_plugin_settings_window(
    app: AppHandle,
    state: tauri::State<'_, WidgetState>,
) -> Result<(), String> {
    if let Ok(mut active) = state.plugin_settings.lock() {
        *active = None;
    }
    if let Some(window) = app.get_webview_window("plugin-settings") {
        window
            .hide()
            .map_err(|error| format!("无法隐藏插件设置窗口：{error}"))?;
    }
    Ok(())
}

#[tauri::command]
pub(crate) fn get_active_plugin_settings_id(
    state: tauri::State<'_, WidgetState>,
) -> Result<Option<String>, String> {
    state
        .plugin_settings
        .lock()
        .map(|active| active.clone())
        .map_err(|error| format!("无法读取插件设置窗口状态：{error}"))
}

fn validate_plugin_permission(state: &WidgetState, plugin_id: &str, permission: &str) -> Result<(), String> {
    let manifests = scan_manifests(&state.plugins_dir)?;
    let manifest = manifests
        .iter()
        .find_map(|(_, manifest)| (manifest.id == plugin_id).then_some(manifest))
        .ok_or_else(|| format!("找不到插件：{plugin_id}"))?;
    if !manifest.permissions.iter().any(|item| item == permission) {
        return Err(format!("插件 {plugin_id} 没有 {permission} 权限"));
    }
    let runtime = load_runtime_config(&state.plugin_config_path);
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
pub(crate) fn plugin_fs_read_text(
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
pub(crate) fn plugin_fs_read_bytes(
    state: tauri::State<'_, WidgetState>,
    plugin_id: String,
    path: String,
) -> Result<Vec<u8>, String> {
    validate_plugin_permission(state.inner(), &plugin_id, "fs.read")?;
    let path = expand_user_path(&path)?;
    fs::read(&path).map_err(|error| format!("无法读取文件 {}：{error}", path.display()))
}

#[tauri::command]
pub(crate) async fn plugin_http_request(
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

/// 从宿主 SQLite 读取插件 JSON 数据。
#[tauri::command]
pub(crate) fn plugin_storage_get(
    state: tauri::State<'_, WidgetState>,
    plugin_id: String,
    key: String,
) -> Result<Option<Value>, String> {
    validate_storage_plugin(state.inner(), &plugin_id)?;
    let connection = Connection::open(&state.db_path)
        .map_err(|error| format!("无法打开插件存储数据库：{error}"))?;
    let json = connection
        .query_row(
            "SELECT json_value FROM plugin_storage WHERE plugin_id = ?1 AND storage_key = ?2",
            params![plugin_id, key],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|error| format!("无法读取插件存储：{error}"))?;
    json.map(|content| {
        serde_json::from_str(&content).map_err(|error| format!("插件存储 JSON 已损坏：{error}"))
    })
    .transpose()
}

/// 将插件 JSON 数据写入宿主 SQLite。
#[tauri::command]
pub(crate) fn plugin_storage_set(
    app: AppHandle,
    state: tauri::State<'_, WidgetState>,
    plugin_id: String,
    key: String,
    value: Value,
) -> Result<(), String> {
    validate_storage_plugin(state.inner(), &plugin_id)?;
    let content = serde_json::to_string(&value)
        .map_err(|error| format!("无法序列化插件存储数据：{error}"))?;
    let connection = Connection::open(&state.db_path)
        .map_err(|error| format!("无法打开插件存储数据库：{error}"))?;
    connection
        .execute(
            "INSERT INTO plugin_storage (plugin_id, storage_key, json_value, updated_at) VALUES (?1, ?2, ?3, ?4) \
             ON CONFLICT(plugin_id, storage_key) DO UPDATE SET json_value = excluded.json_value, updated_at = excluded.updated_at",
            params![plugin_id, key, content, unix_timestamp()],
        )
        .map_err(|error| format!("无法保存插件存储：{error}"))?;
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

/// 删除单个插件存储键。
#[tauri::command]
pub(crate) fn plugin_storage_remove(
    state: tauri::State<'_, WidgetState>,
    plugin_id: String,
    key: String,
) -> Result<(), String> {
    validate_storage_plugin(state.inner(), &plugin_id)?;
    let connection = Connection::open(&state.db_path)
        .map_err(|error| format!("无法打开插件存储数据库：{error}"))?;
    connection
        .execute(
            "DELETE FROM plugin_storage WHERE plugin_id = ?1 AND storage_key = ?2",
            params![plugin_id, key],
        )
        .map_err(|error| format!("无法删除插件存储：{error}"))?;
    Ok(())
}

/// 清空一个插件自己的持久化数据，不影响其他插件。
#[tauri::command]
pub(crate) fn plugin_storage_clear(
    state: tauri::State<'_, WidgetState>,
    plugin_id: String,
) -> Result<(), String> {
    validate_storage_plugin(state.inner(), &plugin_id)?;
    let connection = Connection::open(&state.db_path)
        .map_err(|error| format!("无法打开插件存储数据库：{error}"))?;
    connection
        .execute("DELETE FROM plugin_storage WHERE plugin_id = ?1", params![plugin_id])
        .map_err(|error| format!("无法清空插件存储：{error}"))?;
    Ok(())
}



/// 系统资源监视是内置特殊能力：仅 system-monitor 插件可调用。
#[tauri::command]
pub(crate) fn plugin_system_metrics(
    state: tauri::State<'_, WidgetState>,
    plugin_id: String,
) -> Result<crate::system_monitor::SystemMetrics, String> {
    if plugin_id != "system-monitor" {
        return Err("system.metrics 仅提供给内置系统资源监视插件".to_string());
    }
    validate_plugin_permission(state.inner(), &plugin_id, "system.metrics")?;
    state
        .system_monitor
        .lock()
        .map_err(|error| format!("无法锁定系统资源采样器：{error}"))?
        .sample()
}
