use tauri::{AppHandle, Emitter, LogicalSize, Manager, PhysicalPosition};

#[cfg(target_os = "windows")]
use windows_sys::Win32::{
    Foundation::{HWND, POINT, RECT},
    Graphics::Gdi::{GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST},
    UI::WindowsAndMessaging::GetWindowRect,
};

use crate::WidgetState;
use super::{load_registered_manifests, load_runtime_config};

const PLUGIN_POPUP_INITIAL_WIDTH: f64 = 160.0;
const PLUGIN_POPUP_INITIAL_HEIGHT: f64 = 88.0;
const PLUGIN_POPUP_GAP: f64 = 8.0;

fn validate_popup_plugin(state: &WidgetState, plugin_id: &str) -> Result<(), String> {
    let manifests = load_registered_manifests(state)?;
    let manifest = manifests
        .iter()
        .find_map(|(_, manifest)| (manifest.id == plugin_id).then_some(manifest))
        .ok_or_else(|| format!("找不到插件：{plugin_id}"))?;
    if manifest.popup.is_none() {
        return Err(format!("插件 {plugin_id} 没有 popup"));
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

fn validate_settings_plugin(state: &WidgetState, plugin_id: &str) -> Result<(), String> {
    let manifests = load_registered_manifests(state)?;
    let manifest = manifests
        .iter()
        .find_map(|(_, manifest)| (manifest.id == plugin_id).then_some(manifest))
        .ok_or_else(|| format!("找不到插件：{plugin_id}"))?;
    if manifest.settings.is_none() {
        return Err(format!("插件 {plugin_id} 没有 settings"));
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
pub fn show_plugin_popup(
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
        .show()
        .map_err(|error| format!("无法显示插件弹窗：{error}"))?;
    popup
        .emit("plugin-popup-changed", plugin_id)
        .map_err(|error| format!("无法通知插件弹窗切换：{error}"))?;
    popup
        .set_focus()
        .map_err(|error| format!("无法聚焦插件弹窗：{error}"))?;
    Ok(())
}

#[tauri::command]
pub fn resize_plugin_popup(
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
pub fn hide_plugin_popup(
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
pub fn get_active_plugin_popup_id(
    state: tauri::State<'_, WidgetState>,
) -> Result<Option<String>, String> {
    state
        .plugin_popup
        .lock()
        .map(|active| active.as_ref().map(|item| item.plugin_id.clone()))
        .map_err(|error| format!("无法读取插件弹窗状态：{error}"))
}

#[tauri::command]
pub fn show_plugin_settings_window(
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
    // 先保持隐藏，等 settings.html 完成首轮尺寸计算后再显示，避免旧内容/初始尺寸闪烁。
    let _ = window.hide();
    window
        .emit("plugin-settings-changed", plugin_id)
        .map_err(|error| format!("无法通知插件设置窗口切换：{error}"))?;
    Ok(())
}

#[tauri::command]
pub fn present_plugin_settings_window(app: AppHandle) -> Result<(), String> {
    let window = app
        .get_webview_window("plugin-settings")
        .ok_or_else(|| "找不到 plugin-settings 窗口".to_string())?;
    window
        .set_always_on_top(false)
        .map_err(|error| format!("无法取消插件设置窗口置顶：{error}"))?;
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
pub fn resize_plugin_settings_window(
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
pub fn hide_plugin_settings_window(
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
pub fn get_active_plugin_settings_id(
    state: tauri::State<'_, WidgetState>,
) -> Result<Option<String>, String> {
    state
        .plugin_settings
        .lock()
        .map(|active| active.clone())
        .map_err(|error| format!("无法读取插件设置窗口状态：{error}"))
}

