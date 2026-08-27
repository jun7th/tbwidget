use std::{ptr, thread, time::Duration};

use crate::WidgetPosition;
use tauri::WebviewWindow;
use windows_sys::Win32::{
    Foundation::{GetLastError, SetLastError, HWND, POINT, RECT},
    Graphics::Gdi::ScreenToClient,
    UI::{
        HiDpi::GetDpiForWindow,
        WindowsAndMessaging::{
            FindWindowExW, FindWindowW, GetClientRect, GetParent, GetWindowLongPtrW, GetWindowRect,
            IsWindow, SetParent, SetWindowLongPtrW, SetWindowPos, ShowWindow, GWL_EXSTYLE, GWL_STYLE,
            HWND_TOP, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_SHOWWINDOW, SW_SHOWNOACTIVATE,
            WS_CAPTION, WS_CHILD, WS_CLIPCHILDREN, WS_CLIPSIBLINGS, WS_EX_APPWINDOW,
            WS_EX_TOOLWINDOW, WS_MAXIMIZEBOX, WS_MINIMIZEBOX, WS_POPUP, WS_SYSMENU, WS_THICKFRAME,
            WS_VISIBLE,
        },
    },
};

const FALLBACK_LOGICAL_WIDTH: i32 = 260;
const MIN_LOGICAL_WIDTH: i32 = 120;
const MAX_LOGICAL_WIDTH: i32 = 800;

/// 将 Tauri 主窗口按配置位置挂载到 Windows 主任务栏并设置初始尺寸。
///
/// # Errors
/// 当任务栏查找、窗口样式修改、父窗口设置或尺寸调整失败时返回中文错误信息。
pub fn attach(window: &WebviewWindow, position: WidgetPosition) -> Result<(), String> {
    let native = window
        .hwnd()
        .map_err(|error| format!("无法获取组件窗口句柄：{error}"))?;
    let widget_hwnd = native.0 as HWND;
    let taskbar_hwnd = find_primary_taskbar()?;

    unsafe {
        let mut taskbar_rect: RECT = std::mem::zeroed();
        if GetClientRect(taskbar_hwnd, &mut taskbar_rect) == 0 {
            return Err(last_error("无法读取任务栏尺寸"));
        }

        let taskbar_width = taskbar_rect.right - taskbar_rect.left;
        let taskbar_height = taskbar_rect.bottom - taskbar_rect.top;
        if taskbar_width <= 0 || taskbar_height <= 0 {
            return Err("任务栏客户区尺寸无效".to_string());
        }

        let dpi = GetDpiForWindow(taskbar_hwnd).max(96);
        let widget_width =
            logical_to_physical_width(FALLBACK_LOGICAL_WIDTH, dpi).min(taskbar_width);
        let widget_x = resolve_widget_x(taskbar_hwnd, taskbar_width, widget_width, position);

        let original_style = GetWindowLongPtrW(widget_hwnd, GWL_STYLE);
        let original_ex_style = GetWindowLongPtrW(widget_hwnd, GWL_EXSTYLE);

        let child_style = ((original_style as u32
            & !(WS_POPUP
                | WS_CAPTION
                | WS_THICKFRAME
                | WS_MINIMIZEBOX
                | WS_MAXIMIZEBOX
                | WS_SYSMENU))
            | WS_CHILD
            | WS_VISIBLE
            | WS_CLIPSIBLINGS
            | WS_CLIPCHILDREN) as isize;

        let child_ex_style =
            ((original_ex_style as u32 & !WS_EX_APPWINDOW) | WS_EX_TOOLWINDOW) as isize;

        set_window_style(widget_hwnd, GWL_STYLE, child_style, "无法设置子窗口样式")?;
        if let Err(error) = set_window_style(
            widget_hwnd,
            GWL_EXSTYLE,
            child_ex_style,
            "无法设置子窗口扩展样式",
        ) {
            let _ = set_window_style(widget_hwnd, GWL_STYLE, original_style, "恢复窗口样式失败");
            return Err(error);
        }

        SetLastError(0);
        let previous_parent = SetParent(widget_hwnd, taskbar_hwnd);
        let parent_error = GetLastError();
        if previous_parent.is_null() && parent_error != 0 {
            let _ = set_window_style(widget_hwnd, GWL_STYLE, original_style, "恢复窗口样式失败");
            let _ = set_window_style(
                widget_hwnd,
                GWL_EXSTYLE,
                original_ex_style,
                "恢复窗口扩展样式失败",
            );
            return Err(format!(
                "无法挂载到 Win11 任务栏，Win32 错误码：{parent_error}"
            ));
        }

        if SetWindowPos(
            widget_hwnd,
            HWND_TOP,
            widget_x,
            0,
            widget_width,
            taskbar_height,
            SWP_NOACTIVATE | SWP_FRAMECHANGED | SWP_SHOWWINDOW,
        ) == 0
        {
            let error = last_error("无法定位任务栏组件");
            let _ = SetParent(widget_hwnd, previous_parent);
            let _ = set_window_style(widget_hwnd, GWL_STYLE, original_style, "恢复窗口样式失败");
            let _ = set_window_style(
                widget_hwnd,
                GWL_EXSTYLE,
                original_ex_style,
                "恢复窗口扩展样式失败",
            );
            return Err(error);
        }

        ShowWindow(widget_hwnd, SW_SHOWNOACTIVATE);

        if GetParent(widget_hwnd) != taskbar_hwnd {
            let _ = SetParent(widget_hwnd, previous_parent);
            let _ = set_window_style(widget_hwnd, GWL_STYLE, original_style, "恢复窗口样式失败");
            let _ = set_window_style(
                widget_hwnd,
                GWL_EXSTYLE,
                original_ex_style,
                "恢复窗口扩展样式失败",
            );
            return Err("窗口已定位，但未成为任务栏的子窗口".to_string());
        }

        println!(
            "taskbar widget attached: width={widget_width}px height={taskbar_height}px dpi={dpi}"
        );
    }

    Ok(())
}

/// 根据 Vue 页面提供的逻辑宽度和配置位置调整任务栏子窗口。
///
/// # Errors
/// 当宽度无效、窗口未挂载或 Win32 尺寸调整失败时返回中文错误信息。
pub fn set_width(
    window: &WebviewWindow,
    logical_width: f64,
    position: WidgetPosition,
) -> Result<i32, String> {
    if !logical_width.is_finite() {
        return Err("页面宽度不是有效数字".to_string());
    }

    let logical_width = logical_width
        .round()
        .clamp(MIN_LOGICAL_WIDTH as f64, MAX_LOGICAL_WIDTH as f64) as i32;
    let native = window
        .hwnd()
        .map_err(|error| format!("无法获取组件窗口句柄：{error}"))?;
    let widget_hwnd = native.0 as HWND;

    let (taskbar_hwnd, _) = ensure_current_primary_taskbar(window, widget_hwnd, position)?;
    unsafe {
        place_widget(widget_hwnd, taskbar_hwnd, logical_width, position)?;
    }

    Ok(logical_width)
}

/// 使用当前任务栏 DPI 和组件物理宽度重新计算并应用窗口位置。
///
/// # Errors
/// 当窗口、任务栏或当前组件尺寸无效，或重新挂载/定位失败时返回中文错误信息。
pub fn reflow(window: &WebviewWindow, position: WidgetPosition) -> Result<(), String> {
    let native = window
        .hwnd()
        .map_err(|error| format!("无法获取组件窗口句柄：{error}"))?;
    let widget_hwnd = native.0 as HWND;
    let (taskbar_hwnd, was_current) =
        ensure_current_primary_taskbar(window, widget_hwnd, position)?;

    unsafe {
        let logical_width = if was_current {
            let mut widget_rect: RECT = std::mem::zeroed();
            if GetWindowRect(widget_hwnd, &mut widget_rect) == 0 {
                return Err(last_error("无法读取任务栏组件当前尺寸"));
            }
            let physical_width = widget_rect.right - widget_rect.left;
            if physical_width <= 0 {
                return Err("任务栏组件当前宽度无效".to_string());
            }
            physical_to_logical_width(physical_width, GetDpiForWindow(taskbar_hwnd).max(96))
                .clamp(MIN_LOGICAL_WIDTH, MAX_LOGICAL_WIDTH)
        } else {
            FALLBACK_LOGICAL_WIDTH
        };
        place_widget(widget_hwnd, taskbar_hwnd, logical_width, position)?;
    }

    Ok(())
}

fn should_reattach_to_primary_taskbar(
    parent: isize,
    parent_valid: bool,
    primary: isize,
    primary_valid: bool,
) -> bool {
    !parent_valid || !primary_valid || parent == 0 || parent != primary
}

/// 确保组件当前挂载到此刻的主任务栏，并返回重新读取过的主任务栏句柄。
fn ensure_current_primary_taskbar(
    window: &WebviewWindow,
    widget_hwnd: HWND,
    position: WidgetPosition,
) -> Result<(HWND, bool), String> {
    unsafe {
        if widget_hwnd.is_null() || IsWindow(widget_hwnd) == 0 {
            return Err("任务栏组件窗口句柄已失效".to_string());
        }

        let primary = find_primary_taskbar()?;
        let parent = GetParent(widget_hwnd);
        let needs_reattach = should_reattach_to_primary_taskbar(
            parent as isize,
            !parent.is_null() && IsWindow(parent) != 0,
            primary as isize,
            !primary.is_null() && IsWindow(primary) != 0,
        );
        if !needs_reattach {
            return Ok((primary, true));
        }
    }

    attach(window, position)?;

    unsafe {
        let current_primary = find_primary_taskbar()?;
        let current_parent = GetParent(widget_hwnd);
        if widget_hwnd.is_null()
            || IsWindow(widget_hwnd) == 0
            || should_reattach_to_primary_taskbar(
                current_parent as isize,
                !current_parent.is_null() && IsWindow(current_parent) != 0,
                current_primary as isize,
                !current_primary.is_null() && IsWindow(current_primary) != 0,
            )
        {
            return Err("组件重新挂载后未成为当前主任务栏的有效子窗口".to_string());
        }
        Ok((current_primary, false))
    }
}

/// 将逻辑宽度定位到当前任务栏客户区，并复用同一套物理像素计算。
unsafe fn place_widget(
    widget_hwnd: HWND,
    taskbar_hwnd: HWND,
    logical_width: i32,
    position: WidgetPosition,
) -> Result<(), String> {
    let mut taskbar_rect: RECT = std::mem::zeroed();
    if GetClientRect(taskbar_hwnd, &mut taskbar_rect) == 0 {
        return Err(last_error("无法读取任务栏尺寸"));
    }

    let taskbar_width = taskbar_rect.right - taskbar_rect.left;
    let taskbar_height = taskbar_rect.bottom - taskbar_rect.top;
    if taskbar_width <= 0 || taskbar_height <= 0 {
        return Err("任务栏客户区尺寸无效".to_string());
    }
    let dpi = GetDpiForWindow(taskbar_hwnd).max(96);
    let physical_width = logical_to_physical_width(logical_width, dpi).min(taskbar_width);
    let widget_x = resolve_widget_x(taskbar_hwnd, taskbar_width, physical_width, position);

    if SetWindowPos(
        widget_hwnd,
        HWND_TOP,
        widget_x,
        0,
        physical_width,
        taskbar_height,
        SWP_NOACTIVATE | SWP_SHOWWINDOW,
    ) == 0
    {
        return Err(last_error("无法应用页面宽度"));
    }
    Ok(())
}

/// 根据配置位置计算组件在任务栏客户区中的横坐标。
/// 靠左时固定返回零；靠右时避让当前主任务栏的直接通知区域子窗口。
/// 查找、坐标转换或尺寸校验失败时记录中文日志并安全回退到靠左位置。
fn resolve_widget_x(
    taskbar_hwnd: HWND,
    taskbar_width: i32,
    widget_width: i32,
    position: WidgetPosition,
) -> i32 {
    if position == WidgetPosition::Left {
        return 0;
    }

    let tray_class_name: Vec<u16> = "TrayNotifyWnd\0".encode_utf16().collect();
    unsafe {
        let tray_hwnd = FindWindowExW(
            taskbar_hwnd,
            ptr::null_mut(),
            tray_class_name.as_ptr(),
            ptr::null(),
        );
        if tray_hwnd.is_null() {
            eprintln!("无法找到主任务栏的直接通知区域 TrayNotifyWnd，组件将回退到靠左位置");
            return 0;
        }

        let mut tray_rect: RECT = std::mem::zeroed();
        if GetWindowRect(tray_hwnd, &mut tray_rect) == 0 {
            eprintln!(
                "无法读取任务栏通知区域尺寸，组件将回退到靠左位置：{}",
                last_error("GetWindowRect 调用失败")
            );
            return 0;
        }
        if tray_rect.right <= tray_rect.left || tray_rect.bottom <= tray_rect.top {
            eprintln!("任务栏通知区域尺寸无效，组件将回退到靠左位置");
            return 0;
        }

        let mut tray_left = POINT {
            x: tray_rect.left,
            y: tray_rect.top,
        };
        SetLastError(0);
        if ScreenToClient(taskbar_hwnd, &mut tray_left) == 0 {
            let error = GetLastError();
            eprintln!(
                "无法将任务栏通知区域坐标转换到客户区，组件将回退到靠左位置，Win32 错误码：{error}"
            );
            return 0;
        }

        let available_width = tray_left.x;
        if tray_left.x < 0
            || tray_left.x > taskbar_width
            || widget_width <= 0
            || available_width < widget_width
        {
            eprintln!(
                "任务栏通知区域坐标或可用宽度无效，组件将回退到靠左位置：x={}，任务栏宽度={}，组件宽度={}",
                tray_left.x, taskbar_width, widget_width
            );
            return 0;
        }

        tray_left.x - widget_width
    }
}

/// 将逻辑宽度按照任务栏 DPI 换算为物理像素。
fn logical_to_physical_width(logical_width: i32, dpi: u32) -> i32 {
    ((logical_width as i64 * dpi as i64 + 48) / 96) as i32
}

/// 将物理像素宽度按照当前任务栏 DPI 还原为逻辑宽度。
fn physical_to_logical_width(physical_width: i32, dpi: u32) -> i32 {
    let dpi = dpi.max(1) as i64;
    ((physical_width as i64 * 96 + dpi / 2) / dpi) as i32
}

/// 查找 Windows 主任务栏窗口，并在 Explorer 尚未就绪时进行短暂重试。
///
/// # Errors
/// 当重试后仍找不到 Shell_TrayWnd 时返回中文错误信息。
fn find_primary_taskbar() -> Result<HWND, String> {
    let class_name: Vec<u16> = "Shell_TrayWnd\0".encode_utf16().collect();

    for _ in 0..20 {
        let hwnd = unsafe { FindWindowW(class_name.as_ptr(), ptr::null()) };
        if !hwnd.is_null() {
            return Ok(hwnd);
        }
        thread::sleep(Duration::from_millis(100));
    }

    Err("找不到主任务栏窗口 Shell_TrayWnd".to_string())
}

/// 设置指定窗口样式，并正确处理 Win32 返回零值的歧义。
///
/// # Errors
/// 当 SetWindowLongPtrW 返回 Win32 错误时返回中文错误信息。
unsafe fn set_window_style(
    hwnd: HWND,
    index: i32,
    value: isize,
    context: &str,
) -> Result<(), String> {
    SetLastError(0);
    let previous = SetWindowLongPtrW(hwnd, index, value);
    let error = GetLastError();
    if previous == 0 && error != 0 {
        return Err(format!("{context}，Win32 错误码：{error}"));
    }
    Ok(())
}

/// 将当前 Win32 错误码与操作上下文组合为中文错误信息。
fn last_error(context: &str) -> String {
    let error = unsafe { GetLastError() };
    format!("{context}，Win32 错误码：{error}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn width_scales_for_common_dpi_values() {
        assert_eq!(logical_to_physical_width(260, 96), 260);
        assert_eq!(logical_to_physical_width(260, 144), 390);
        assert_eq!(logical_to_physical_width(260, 192), 520);
    }

    #[test]
    fn physical_width_recovers_logical_width_at_current_dpi() {
        assert_eq!(physical_to_logical_width(260, 96), 260);
        assert_eq!(physical_to_logical_width(390, 144), 260);
        assert_eq!(physical_to_logical_width(520, 192), 260);
    }

    #[test]
    fn stale_or_invalid_taskbar_parent_requires_reattach() {
        assert!(!should_reattach_to_primary_taskbar(10, true, 10, true));
        assert!(should_reattach_to_primary_taskbar(9, true, 10, true));
        assert!(should_reattach_to_primary_taskbar(10, false, 10, true));
        assert!(should_reattach_to_primary_taskbar(10, true, 10, false));
    }
}
