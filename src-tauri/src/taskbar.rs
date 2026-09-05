use std::{
    collections::{HashSet, VecDeque},
    ffi::c_void,
    ptr,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc, Mutex, OnceLock,
    },
    thread,
    time::Duration,
};

use crate::WidgetPosition;
use tauri::WebviewWindow;
use windows_sys::Win32::{
    Foundation::{GetLastError, SetLastError, HWND, POINT, RECT},
    System::LibraryLoader::GetModuleHandleW,
    UI::{
        HiDpi::GetDpiForWindow,
        WindowsAndMessaging::{
            FindWindowExW, FindWindowW, GetClassNameW, GetClientRect, GetCursorPos,
            GetParent, GetTopWindow, GetWindow, GetWindowLongPtrW, GetWindowRect,
            IsWindow, IsWindowVisible, SetParent, SetWindowLongPtrW, SetWindowPos, ShowWindow,
            WindowFromPoint, GWL_EXSTYLE, GWL_STYLE, GW_HWNDNEXT, HWND_TOP, SWP_FRAMECHANGED,
            SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOOWNERZORDER, SWP_NOSIZE, SWP_SHOWWINDOW,
            SW_SHOWNOACTIVATE, WS_CAPTION, WS_CHILD, WS_CLIPCHILDREN, WS_CLIPSIBLINGS,
            WS_EX_APPWINDOW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT,
            WS_DISABLED, WS_MAXIMIZEBOX, WS_MINIMIZEBOX, WS_POPUP, WS_SYSMENU, WS_THICKFRAME,
            WS_VISIBLE, MSG,
        },
    },
};

const FALLBACK_LOGICAL_WIDTH: i32 = 260;
const MIN_LOGICAL_WIDTH: i32 = 120;
const MAX_LOGICAL_WIDTH: i32 = 800;

// Shell_TrayWnd 是父窗口时，未处理的 WM_CONTEXTMENU 会被 DefWindowProc 转交给任务栏。
// 对组件 HWND 做轻量 subclass：吞掉系统上下文菜单，并拒绝 HTTRANSPARENT 命中结果。
const WM_MOUSEACTIVATE_MESSAGE: u32 = 0x0021;
const WM_MOUSEMOVE_MESSAGE: u32 = 0x0200;
const WM_LBUTTONDOWN_MESSAGE: u32 = 0x0201;
const WM_LBUTTONUP_MESSAGE: u32 = 0x0202;
const WM_RBUTTONDOWN_MESSAGE: u32 = 0x0204;
const WM_RBUTTONUP_MESSAGE: u32 = 0x0205;
const WM_MBUTTONDOWN_MESSAGE: u32 = 0x0207;
const WM_MBUTTONUP_MESSAGE: u32 = 0x0208;
const WM_NCHITTEST_MESSAGE: u32 = 0x0084;
const WM_CONTEXTMENU_MESSAGE: u32 = 0x007B;
const HTCLIENT_RESULT: isize = 1;
const MA_NOACTIVATE_RESULT: isize = 3;
const WIDGET_INPUT_SUBCLASS_ID: usize = 0x5442_5749; // "TBWI"
const SUBCLASS_ROLE_WIDGET: usize = 1;
const SUBCLASS_ROLE_WEBVIEW: usize = 2;
const DESCENDANT_SCAN_DEPTH: usize = 8;
const DESCENDANT_SCAN_LIMIT: usize = 64;

#[derive(Default)]
struct ProbeState {
    last_signature: String,
}

static PROBE_STATE: OnceLock<Mutex<ProbeState>> = OnceLock::new();
static NATIVE_EVENT_QUEUE: OnceLock<Mutex<VecDeque<String>>> = OnceLock::new();
static NATIVE_LEFT_DOWN_QUEUE: OnceLock<Mutex<VecDeque<NativePointerClick>>> = OnceLock::new();
static NATIVE_RIGHT_CLICK_QUEUE: OnceLock<Mutex<VecDeque<NativePointerClick>>> = OnceLock::new();
static MOUSE_LEFT_BUTTON_DOWN: AtomicBool = AtomicBool::new(false);
static SUBCLASSED_WINDOWS: OnceLock<Mutex<HashSet<usize>>> = OnceLock::new();
static MOUSE_HOOK_RUNNING: AtomicBool = AtomicBool::new(false);
static MOUSE_HOOK_WIDGET_HWND: AtomicUsize = AtomicUsize::new(0);
static INTERACTIVE_REGIONS: OnceLock<Mutex<Vec<PhysicalInteractiveRegion>>> = OnceLock::new();
const INPUT_DEBUG_LOGS: bool = false;

#[derive(Clone)]
struct PhysicalInteractiveRegion {
    id: String,
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

#[derive(Clone, Copy)]
pub(crate) struct NativePointerPoint {
    pub(crate) x: f64,
    pub(crate) y: f64,
}

#[derive(Clone)]
pub(crate) struct NativePointerClick {
    pub(crate) x: f64,
    pub(crate) y: f64,
    pub(crate) region_id: String,
}

#[derive(Clone)]
pub(crate) struct InteractiveRegion {
    pub(crate) id: String,
    pub(crate) x: f64,
    pub(crate) y: f64,
    pub(crate) width: f64,
    pub(crate) height: f64,
}

fn record_native_event(message: String) {
    if !crate::debug_logging_enabled() {
        return;
    }
    let queue = NATIVE_EVENT_QUEUE.get_or_init(|| Mutex::new(VecDeque::with_capacity(64)));
    if let Ok(mut queue) = queue.try_lock() {
        if queue.len() >= 64 {
            queue.pop_front();
        }
        queue.push_back(message);
    }
}

pub(crate) fn take_native_events() -> Vec<String> {
    NATIVE_EVENT_QUEUE
        .get_or_init(|| Mutex::new(VecDeque::with_capacity(64)))
        .lock()
        .map(|mut queue| queue.drain(..).collect())
        .unwrap_or_default()
}


pub(crate) fn take_native_left_downs() -> Vec<NativePointerClick> {
    NATIVE_LEFT_DOWN_QUEUE
        .get_or_init(|| Mutex::new(VecDeque::with_capacity(16)))
        .lock()
        .map(|mut queue| queue.drain(..).collect())
        .unwrap_or_default()
}

pub(crate) fn take_native_right_clicks() -> Vec<NativePointerClick> {
    NATIVE_RIGHT_CLICK_QUEUE
        .get_or_init(|| Mutex::new(VecDeque::with_capacity(16)))
        .lock()
        .map(|mut queue| queue.drain(..).collect())
        .unwrap_or_default()
}

pub(crate) fn native_left_button_down() -> bool {
    MOUSE_LEFT_BUTTON_DOWN.load(Ordering::Relaxed)
}

pub(crate) fn native_cursor_position() -> Option<NativePointerPoint> {
    let widget_hwnd = MOUSE_HOOK_WIDGET_HWND.load(Ordering::Relaxed) as HWND;
    if widget_hwnd.is_null() {
        return None;
    }
    unsafe {
        if IsWindow(widget_hwnd) == 0 {
            return None;
        }
        let mut point = POINT { x: 0, y: 0 };
        let mut rect: RECT = std::mem::zeroed();
        if GetCursorPos(&mut point) == 0 || GetWindowRect(widget_hwnd, &mut rect) == 0 {
            return None;
        }
        if point.x < rect.left || point.x >= rect.right || point.y < rect.top || point.y >= rect.bottom {
            return None;
        }
        let dpi = GetDpiForWindow(widget_hwnd).max(96) as f64;
        Some(NativePointerPoint {
            x: (point.x - rect.left) as f64 * 96.0 / dpi,
            y: (point.y - rect.top) as f64 * 96.0 / dpi,
        })
    }
}

fn interactive_region_at(point: POINT) -> Option<String> {
    INTERACTIVE_REGIONS
        .get_or_init(|| Mutex::new(Vec::new()))
        .lock()
        .ok()
        .and_then(|regions| {
            regions.iter().find_map(|region| {
                (point.x >= region.left
                    && point.x < region.right
                    && point.y >= region.top
                    && point.y < region.bottom)
                    .then(|| region.id.clone())
            })
        })
}

pub(crate) fn native_cursor_region_id() -> Option<String> {
    let widget_hwnd = MOUSE_HOOK_WIDGET_HWND.load(Ordering::Relaxed) as HWND;
    if widget_hwnd.is_null() {
        return None;
    }
    unsafe {
        if IsWindow(widget_hwnd) == 0 {
            return None;
        }
        let mut point = POINT { x: 0, y: 0 };
        let mut rect: RECT = std::mem::zeroed();
        if GetCursorPos(&mut point) == 0 || GetWindowRect(widget_hwnd, &mut rect) == 0 {
            return None;
        }
        if point.x < rect.left || point.x >= rect.right || point.y < rect.top || point.y >= rect.bottom {
            return None;
        }
        interactive_region_at(point)
    }
}

fn queue_native_pointer(queue: &OnceLock<Mutex<VecDeque<NativePointerClick>>>, widget_hwnd: HWND, point: POINT, region_id: String) {
    unsafe {
        let mut rect: RECT = std::mem::zeroed();
        if GetWindowRect(widget_hwnd, &mut rect) == 0 {
            return;
        }
        let dpi = GetDpiForWindow(widget_hwnd).max(96) as f64;
        let click = NativePointerClick {
            x: (point.x - rect.left) as f64 * 96.0 / dpi,
            y: (point.y - rect.top) as f64 * 96.0 / dpi,
            region_id,
        };
        let queue = queue.get_or_init(|| Mutex::new(VecDeque::with_capacity(16)));
        if let Ok(mut queue) = queue.try_lock() {
            if queue.len() >= 16 {
                queue.pop_front();
            }
            queue.push_back(click);
        }
    }
}

type WidgetSubclassProc = unsafe extern "system" fn(HWND, u32, usize, isize, usize, usize) -> isize;
type LowLevelMouseProc = unsafe extern "system" fn(i32, usize, isize) -> isize;
type HHook = *mut c_void;

const WH_MOUSE_LL: i32 = 14;
const HC_ACTION: i32 = 0;

#[repr(C)]
struct MsllHookStruct {
    pt: POINT,
    mouse_data: u32,
    flags: u32,
    time: u32,
    extra_info: usize,
}

#[link(name = "comctl32")]
extern "system" {
    fn SetWindowSubclass(
        hwnd: HWND,
        subclass_proc: WidgetSubclassProc,
        subclass_id: usize,
        ref_data: usize,
    ) -> i32;
    fn DefSubclassProc(hwnd: HWND, message: u32, wparam: usize, lparam: isize) -> isize;
}

#[link(name = "user32")]
extern "system" {
    fn GetWindowThreadProcessId(hwnd: HWND, process_id: *mut u32) -> u32;
    fn SetWindowsHookExW(
        id_hook: i32,
        hook_proc: Option<LowLevelMouseProc>,
        module: *mut c_void,
        thread_id: u32,
    ) -> HHook;
    fn CallNextHookEx(
        hook: HHook,
        code: i32,
        wparam: usize,
        lparam: isize,
    ) -> isize;
    fn GetMessageW(message: *mut MSG, hwnd: HWND, min_filter: u32, max_filter: u32) -> i32;
    fn UnhookWindowsHookEx(hook: HHook) -> i32;
}

#[link(name = "kernel32")]
extern "system" {
    fn GetCurrentThreadId() -> u32;
}

unsafe extern "system" fn low_level_mouse_proc(
    code: i32,
    wparam: usize,
    lparam: isize,
) -> isize {
    let message = wparam as u32;
    if code == HC_ACTION && is_mouse_button_message(message) && lparam != 0 {
        let ending_drag = message == WM_LBUTTONUP_MESSAGE
            && MOUSE_LEFT_BUTTON_DOWN.swap(false, Ordering::Relaxed);

        let event = &*(lparam as *const MsllHookStruct);
        let widget_hwnd = MOUSE_HOOK_WIDGET_HWND.load(Ordering::Relaxed) as HWND;

        if !widget_hwnd.is_null() && IsWindow(widget_hwnd) != 0 {
            let mut rect: RECT = std::mem::zeroed();
            if GetWindowRect(widget_hwnd, &mut rect) != 0 {
                let inside = event.pt.x >= rect.left
                    && event.pt.x < rect.right
                    && event.pt.y >= rect.top
                    && event.pt.y < rect.bottom;

                if inside {
                    let region_id = interactive_region_at(event.pt);
                    if INPUT_DEBUG_LOGS {
                        let hit_hwnd = WindowFromPoint(event.pt);
                        let hit_class = window_class_name(hit_hwnd);
                        let (button, state) = low_level_button_state(message);
                        record_native_event(format!(
                            "mouse {} {} pos=({}, {}) interactive={} region={} hit={}",
                            button,
                            state,
                            event.pt.x,
                            event.pt.y,
                            region_id.is_some(),
                            region_id.as_deref().unwrap_or("<none>"),
                            hit_class,
                        ));
                    }
                    if let Some(region_id) = region_id {
                        match message {
                            WM_LBUTTONDOWN_MESSAGE => {
                                MOUSE_LEFT_BUTTON_DOWN.store(true, Ordering::Relaxed);
                                queue_native_pointer(&NATIVE_LEFT_DOWN_QUEUE, widget_hwnd, event.pt, region_id);
                            }
                            WM_RBUTTONUP_MESSAGE => {
                                queue_native_pointer(&NATIVE_RIGHT_CLICK_QUEUE, widget_hwnd, event.pt, region_id);
                            }
                            _ => {}
                        }
                        // main WebView 本身保持原生点透；交互区域由低级 Hook 接管，
                        // 阻止同一次点击继续落到 Shell_TrayWnd。
                        return 1;
                    }
                }
            }
        }

        // 左键按下由本窗口接管后，即使释放时已经拖出任务栏，也吞掉对应的 UP。
        if ending_drag {
            return 1;
        }
    }

    CallNextHookEx(ptr::null_mut(), code, wparam, lparam)
}

fn is_mouse_button_message(message: u32) -> bool {
    matches!(
        message,
        WM_LBUTTONDOWN_MESSAGE
            | WM_LBUTTONUP_MESSAGE
            | WM_RBUTTONDOWN_MESSAGE
            | WM_RBUTTONUP_MESSAGE
            | WM_MBUTTONDOWN_MESSAGE
            | WM_MBUTTONUP_MESSAGE
    )
}


fn low_level_button_state(message: u32) -> (&'static str, &'static str) {
    match message {
        WM_LBUTTONDOWN_MESSAGE => ("LEFT", "DOWN"),
        WM_LBUTTONUP_MESSAGE => ("LEFT", "UP"),
        WM_RBUTTONDOWN_MESSAGE => ("RIGHT", "DOWN"),
        WM_RBUTTONUP_MESSAGE => ("RIGHT", "UP"),
        WM_MBUTTONDOWN_MESSAGE => ("MIDDLE", "DOWN"),
        WM_MBUTTONUP_MESSAGE => ("MIDDLE", "UP"),
        _ => ("UNKNOWN", "UNKNOWN"),
    }
}

fn ensure_low_level_mouse_hook(widget_hwnd: HWND) -> Result<(), String> {
    MOUSE_HOOK_WIDGET_HWND.store(widget_hwnd as usize, Ordering::Relaxed);

    if MOUSE_HOOK_RUNNING
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Ok(());
    }

    let (ready_tx, ready_rx) = mpsc::sync_channel::<Result<(), String>>(1);
    let spawn_result = thread::Builder::new()
        .name("tbwidget-mouse-hook".to_string())
        .spawn(move || unsafe {
            let module = GetModuleHandleW(ptr::null());
            let hook = SetWindowsHookExW(WH_MOUSE_LL, Some(low_level_mouse_proc), module as *mut c_void, 0);
            if hook.is_null() {
                let error = GetLastError();
                MOUSE_HOOK_RUNNING.store(false, Ordering::Release);
                let _ = ready_tx.send(Err(format!(
                    "无法安装 WH_MOUSE_LL，Win32 错误码：{error}"
                )));
                return;
            }

            let _ = ready_tx.send(Ok(()));
            record_native_event("mouse-hook WH_MOUSE_LL installed".to_string());

            let mut message: MSG = std::mem::zeroed();
            while GetMessageW(&mut message, ptr::null_mut(), 0, 0) > 0 {}

            let _ = UnhookWindowsHookEx(hook);
            MOUSE_HOOK_RUNNING.store(false, Ordering::Release);
        });

    if let Err(error) = spawn_result {
        MOUSE_HOOK_RUNNING.store(false, Ordering::Release);
        return Err(format!("无法创建鼠标 Hook 线程：{error}"));
    }

    match ready_rx.recv_timeout(Duration::from_secs(2)) {
        Ok(result) => result,
        Err(error) => Err(format!("等待 WH_MOUSE_LL 初始化超时：{error}")),
    }
}

unsafe extern "system" fn widget_input_subclass_proc(
    hwnd: HWND,
    message: u32,
    wparam: usize,
    lparam: isize,
    _subclass_id: usize,
    ref_data: usize,
) -> isize {
    match message {
        WM_CONTEXTMENU_MESSAGE => {
            record_native_event(format!(
                "input WM_CONTEXTMENU class={} hwnd={:#x} role={} consumed",
                window_class_name(hwnd),
                hwnd as usize,
                subclass_role_name(ref_data),
            ));
            // 无论消息落在 Tauri 外层还是 WebView2 子 HWND，都不要继续冒泡到 Shell_TrayWnd。
            return 0;
        }
        WM_MOUSEACTIVATE_MESSAGE => {
            record_native_event(format!(
                "input WM_MOUSEACTIVATE class={} hwnd={:#x} role={} -> MA_NOACTIVATE",
                window_class_name(hwnd),
                hwnd as usize,
                subclass_role_name(ref_data),
            ));
            // 不激活 Explorer/任务栏，但保留当前鼠标消息。
            return MA_NOACTIVATE_RESULT;
        }
        WM_NCHITTEST_MESSAGE if ref_data == SUBCLASS_ROLE_WIDGET => {
            // 只对 Tauri 外层强制客户区。WebView2 内部 HWND 保留 Chromium 自己的命中测试。
            return HTCLIENT_RESULT;
        }
        WM_LBUTTONDOWN_MESSAGE
        | WM_LBUTTONUP_MESSAGE
        | WM_RBUTTONDOWN_MESSAGE
        | WM_RBUTTONUP_MESSAGE
        | WM_MBUTTONDOWN_MESSAGE
        | WM_MBUTTONUP_MESSAGE => {
            record_native_event(format!(
                "input {} class={} hwnd={:#x} role={}",
                mouse_message_name(message),
                window_class_name(hwnd),
                hwnd as usize,
                subclass_role_name(ref_data),
            ));
        }
        WM_MOUSEMOVE_MESSAGE => {
            // WM_MOUSEMOVE 频率很高，不逐条输出；命中 HWND 由周期诊断负责。
        }
        _ => {}
    }

    // 除了上面明确消费的消息，全部继续交给 WebView2/Chromium 原始窗口过程。
    DefSubclassProc(hwnd, message, wparam, lparam)
}

fn subclass_role_name(role: usize) -> &'static str {
    match role {
        SUBCLASS_ROLE_WIDGET => "widget",
        SUBCLASS_ROLE_WEBVIEW => "webview",
        _ => "unknown",
    }
}

fn mouse_message_name(message: u32) -> &'static str {
    match message {
        WM_LBUTTONDOWN_MESSAGE => "WM_LBUTTONDOWN",
        WM_LBUTTONUP_MESSAGE => "WM_LBUTTONUP",
        WM_RBUTTONDOWN_MESSAGE => "WM_RBUTTONDOWN",
        WM_RBUTTONUP_MESSAGE => "WM_RBUTTONUP",
        WM_MBUTTONDOWN_MESSAGE => "WM_MBUTTONDOWN",
        WM_MBUTTONUP_MESSAGE => "WM_MBUTTONUP",
        _ => "WM_MOUSE",
    }
}

unsafe fn install_input_guard(hwnd: HWND, role: usize) -> Result<(), String> {
    if hwnd.is_null() || IsWindow(hwnd) == 0 {
        return Err("无法安装鼠标消息拦截器：窗口句柄无效".to_string());
    }

    let caller_thread = GetCurrentThreadId();
    let mut process_id = 0u32;
    let target_thread = GetWindowThreadProcessId(hwnd, &mut process_id);
    if target_thread == 0 {
        return Err(format!(
            "无法读取窗口线程：class={} hwnd={:#x}",
            window_class_name(hwnd),
            hwnd as usize
        ));
    }
    if target_thread != caller_thread {
        return Err(format!(
            "跨线程不可使用 SetWindowSubclass：class={} hwnd={:#x} pid={} target_tid={} caller_tid={}",
            window_class_name(hwnd),
            hwnd as usize,
            process_id,
            target_thread,
            caller_thread
        ));
    }

    if SUBCLASSED_WINDOWS
        .get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
        .map(|registry| registry.contains(&(hwnd as usize)))
        .unwrap_or(false)
    {
        return Ok(());
    }

    if SetWindowSubclass(
        hwnd,
        widget_input_subclass_proc,
        WIDGET_INPUT_SUBCLASS_ID,
        role,
    ) == 0
    {
        return Err(format!(
            "无法安装鼠标消息拦截器：class={} hwnd={:#x} tid={}",
            window_class_name(hwnd),
            hwnd as usize,
            target_thread
        ));
    }

    let registry = SUBCLASSED_WINDOWS.get_or_init(|| Mutex::new(HashSet::new()));
    if let Ok(mut registry) = registry.lock() {
        if registry.insert(hwnd as usize) {
            record_native_event(format!(
                "subclass class={} hwnd={:#x} role={} pid={} tid={}",
                window_class_name(hwnd),
                hwnd as usize,
                subclass_role_name(role),
                process_id,
                target_thread
            ));
        }
    }

    Ok(())
}

unsafe fn collect_descendant_windows(
    parent: HWND,
    remaining_depth: usize,
    limit: usize,
    output: &mut Vec<HWND>,
) {
    if remaining_depth == 0 || output.len() >= limit {
        return;
    }

    let mut child = GetTopWindow(parent);
    while !child.is_null() && IsWindow(child) != 0 && output.len() < limit {
        output.push(child);
        collect_descendant_windows(child, remaining_depth - 1, limit, output);
        child = GetWindow(child, GW_HWNDNEXT);
    }
}


unsafe fn rect_text(rect: &RECT) -> String {
    format!(
        "({},{}-{},{} {}x{})",
        rect.left,
        rect.top,
        rect.right,
        rect.bottom,
        rect.right - rect.left,
        rect.bottom - rect.top
    )
}

unsafe fn describe_window_detailed(hwnd: HWND) -> String {
    if hwnd.is_null() || IsWindow(hwnd) == 0 {
        return format!("invalid hwnd={:#x}", hwnd as usize);
    }

    let parent = GetParent(hwnd);
    let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
    let ex_style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
    let visible = IsWindowVisible(hwnd) != 0;
    let dpi = GetDpiForWindow(hwnd);
    let mut window_rect: RECT = std::mem::zeroed();
    let mut client_rect: RECT = std::mem::zeroed();
    SetLastError(0);
    let window_rect_ok = GetWindowRect(hwnd, &mut window_rect) != 0;
    let window_rect_error = GetLastError();
    SetLastError(0);
    let client_rect_ok = GetClientRect(hwnd, &mut client_rect) != 0;
    let client_rect_error = GetLastError();
    let window_rect_text = if window_rect_ok { rect_text(&window_rect) } else { format!("<GetWindowRect err={window_rect_error}>") };
    let client_rect_text = if client_rect_ok { rect_text(&client_rect) } else { format!("<GetClientRect err={client_rect_error}>") };

    format!(
        "class={} hwnd={:#x} parent={:#x} visible={} dpi={} style=0x{:08X} ex=0x{:08X} flags=[child:{} visible:{} popup:{} exTransparent:{} tool:{} app:{} noActivate:{}] win={} client={}",
        window_class_name(hwnd),
        hwnd as usize,
        parent as usize,
        visible,
        dpi,
        style,
        ex_style,
        style & WS_CHILD != 0,
        style & WS_VISIBLE != 0,
        style & WS_POPUP != 0,
        ex_style & WS_EX_TRANSPARENT != 0,
        ex_style & WS_EX_TOOLWINDOW != 0,
        ex_style & WS_EX_APPWINDOW != 0,
        ex_style & WS_EX_NOACTIVATE != 0,
        window_rect_text,
        client_rect_text
    )
}

/// 输出任务栏窗口、组件外层 HWND 及 WebView2/Chromium 子窗口的完整显示状态。
/// 主要用于区分“SetParent 成功但被遮挡/尺寸异常”和“WebView 内容没有创建/没有显示”。
#[allow(dead_code)]
pub fn diagnostic_snapshot(window: &WebviewWindow, stage: &str) -> Result<Vec<String>, String> {
    if !crate::debug_logging_enabled() {
        return Ok(Vec::new());
    }
    let native = window
        .hwnd()
        .map_err(|error| format!("无法获取组件窗口句柄：{error}"))?;
    let widget_hwnd = native.0 as HWND;
    let taskbar_hwnd = find_primary_taskbar()?;

    unsafe {
        let mut lines = Vec::new();
        lines.push(format!("diag stage={stage} widget [{}]", describe_window_detailed(widget_hwnd)));
        lines.push(format!("diag stage={stage} taskbar [{}]", describe_window_detailed(taskbar_hwnd)));
        lines.push(format!(
            "diag stage={stage} relation parent={:#x} expected={:#x} parent_ok={} top_child={:#x} widget_is_top={}",
            GetParent(widget_hwnd) as usize,
            taskbar_hwnd as usize,
            GetParent(widget_hwnd) == taskbar_hwnd,
            GetTopWindow(taskbar_hwnd) as usize,
            GetTopWindow(taskbar_hwnd) == widget_hwnd
        ));

        let mut descendants = Vec::new();
        collect_descendant_windows(
            widget_hwnd,
            DESCENDANT_SCAN_DEPTH,
            DESCENDANT_SCAN_LIMIT,
            &mut descendants,
        );
        lines.push(format!("diag stage={stage} descendants={}", descendants.len()));
        for (index, hwnd) in descendants.into_iter().enumerate() {
            lines.push(format!(
                "diag stage={stage} child#{index} [{}]",
                describe_window_detailed(hwnd)
            ));
        }

        let mut direct_child = GetTopWindow(taskbar_hwnd);
        for index in 0..16usize {
            if direct_child.is_null() || IsWindow(direct_child) == 0 {
                break;
            }
            lines.push(format!(
                "diag stage={stage} taskbar-child#{index} [{}]{}",
                describe_window_detailed(direct_child),
                if direct_child == widget_hwnd { " <WIDGET>" } else { "" }
            ));
            direct_child = GetWindow(direct_child, GW_HWNDNEXT);
        }

        let mut widget_rect: RECT = std::mem::zeroed();
        if GetWindowRect(widget_hwnd, &mut widget_rect) != 0
            && widget_rect.right > widget_rect.left
            && widget_rect.bottom > widget_rect.top
        {
            let center = POINT {
                x: widget_rect.left + (widget_rect.right - widget_rect.left) / 2,
                y: widget_rect.top + (widget_rect.bottom - widget_rect.top) / 2,
            };
            let hit = WindowFromPoint(center);
            lines.push(format!(
                "diag stage={stage} center-hit point=({}, {}) hit=[{}] chain=[{}]",
                center.x,
                center.y,
                describe_window_compact(hit),
                describe_parent_chain_compact(hit, 8)
            ));
        }

        Ok(lines)
    }
}

unsafe fn is_webview_input_window(hwnd: HWND) -> bool {
    let class_name = window_class_name(hwnd);
    class_name == "WRY_WEBVIEW"
        || class_name.starts_with("Chrome_WidgetWin_")
        || class_name == "Chrome_RenderWidgetHostHWND"
}

/// 给当前已经创建出的 WebView2/Chromium 子 HWND 安装输入拦截。
/// SetWindowSubclass 不能跨线程，因此跨线程窗口只记录诊断，不强行修改。
unsafe fn install_webview_descendant_guards(widget_hwnd: HWND) -> String {
    let caller_thread = GetCurrentThreadId();
    let mut descendants = Vec::new();
    collect_descendant_windows(
        widget_hwnd,
        DESCENDANT_SCAN_DEPTH,
        DESCENDANT_SCAN_LIMIT,
        &mut descendants,
    );

    let mut guarded = Vec::new();
    let mut cross_thread = Vec::new();
    let mut failed = Vec::new();

    for hwnd in descendants.into_iter().filter(|hwnd| is_webview_input_window(*hwnd)) {
        let class_name = window_class_name(hwnd);
        let mut process_id = 0u32;
        let target_thread = GetWindowThreadProcessId(hwnd, &mut process_id);

        if target_thread != caller_thread {
            cross_thread.push(format!(
                "{}({}/{})",
                class_name, process_id, target_thread
            ));
            continue;
        }

        match install_input_guard(hwnd, SUBCLASS_ROLE_WEBVIEW) {
            Ok(()) => guarded.push(class_name),
            Err(error) => failed.push(error),
        }
    }

    let mut parts = vec![format!("guarded=[{}]", guarded.join(","))];
    if !cross_thread.is_empty() {
        parts.push(format!("cross=[{}]", cross_thread.join(",")));
    }
    if !failed.is_empty() {
        parts.push(format!("failed=[{}]", failed.join(" | ")));
    }
    parts.join(" ")
}

/// 将 Tauri 主窗口按配置位置挂载到 Windows 主任务栏并设置初始尺寸。
///
/// # Errors
/// 当任务栏查找、窗口样式修改、父窗口设置或尺寸调整失败时返回中文错误信息。
pub fn attach(window: &WebviewWindow, _position: WidgetPosition) -> Result<(), String> {
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
        // main HWND 始终铺满任务栏并保持原生点透；交互区域由 WH_MOUSE_LL 接管。
        let widget_width = taskbar_width;
        let widget_x = 0;

        let original_style = GetWindowLongPtrW(widget_hwnd, GWL_STYLE);
        let original_ex_style = GetWindowLongPtrW(widget_hwnd, GWL_EXSTYLE);
        record_native_event(format!(
            "attach begin widget={:#x} taskbar={:#x} taskbar_client={} dpi={} original_style=0x{:08X} original_ex=0x{:08X}",
            widget_hwnd as usize,
            taskbar_hwnd as usize,
            rect_text(&taskbar_rect),
            dpi,
            original_style as u32,
            original_ex_style as u32
        ));

        let child_style = ((original_style as u32
            & !(WS_POPUP
                | WS_CAPTION
                | WS_THICKFRAME
                | WS_MINIMIZEBOX
                | WS_MAXIMIZEBOX
                | WS_SYSMENU
                | WS_DISABLED))
            | WS_CHILD
            | WS_VISIBLE
            | WS_CLIPSIBLINGS
            | WS_CLIPCHILDREN) as isize;

        let child_ex_style = ((original_ex_style as u32
            & !(WS_EX_APPWINDOW | WS_EX_NOACTIVATE))
            | WS_EX_TOOLWINDOW
            | WS_EX_TRANSPARENT) as isize;

        set_window_style(widget_hwnd, GWL_STYLE, child_style, "无法设置子窗口样式")?;
        record_native_event(format!(
            "attach style widget={:#x} new_style=0x{:08X} readback=0x{:08X}",
            widget_hwnd as usize,
            child_style as u32,
            GetWindowLongPtrW(widget_hwnd, GWL_STYLE) as u32
        ));
        if let Err(error) = set_window_style(
            widget_hwnd,
            GWL_EXSTYLE,
            child_ex_style,
            "无法设置子窗口扩展样式",
        ) {
            let _ = set_window_style(widget_hwnd, GWL_STYLE, original_style, "恢复窗口样式失败");
            return Err(error);
        }
        record_native_event(format!(
            "attach ex-style widget={:#x} new_ex=0x{:08X} readback=0x{:08X}",
            widget_hwnd as usize,
            child_ex_style as u32,
            GetWindowLongPtrW(widget_hwnd, GWL_EXSTYLE) as u32
        ));

        // 显式清掉 WS_DISABLED；避免依赖当前 windows-sys 未导出的 EnableWindow 绑定。

        if let Err(error) = install_input_guard(widget_hwnd, SUBCLASS_ROLE_WIDGET) {
            let _ = set_window_style(widget_hwnd, GWL_STYLE, original_style, "恢复窗口样式失败");
            let _ = set_window_style(
                widget_hwnd,
                GWL_EXSTYLE,
                original_ex_style,
                "恢复窗口扩展样式失败",
            );
            return Err(error);
        }

        SetLastError(0);
        let previous_parent = SetParent(widget_hwnd, taskbar_hwnd);
        let parent_error = GetLastError();
        record_native_event(format!(
            "attach SetParent widget={:#x} target={:#x} previous={:#x} last_error={} readback_parent={:#x}",
            widget_hwnd as usize,
            taskbar_hwnd as usize,
            previous_parent as usize,
            parent_error,
            GetParent(widget_hwnd) as usize
        ));
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

        SetLastError(0);
        let set_pos_result = SetWindowPos(
            widget_hwnd,
            HWND_TOP,
            widget_x,
            0,
            widget_width,
            taskbar_height,
            SWP_NOACTIVATE | SWP_FRAMECHANGED | SWP_SHOWWINDOW,
        );
        let set_pos_error = GetLastError();
        record_native_event(format!(
            "attach SetWindowPos result={} last_error={} requested=({},0 {}x{}) top_after={:#x}",
            set_pos_result,
            set_pos_error,
            widget_x,
            widget_width,
            taskbar_height,
            GetTopWindow(taskbar_hwnd) as usize
        ));
        if set_pos_result == 0
        {
            let error = format!("无法定位任务栏组件，Win32 错误码：{set_pos_error}");
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

        let show_previous_visible = ShowWindow(widget_hwnd, SW_SHOWNOACTIVATE);
        record_native_event(format!(
            "attach ShowWindow previous_visible={} now_visible={} style=0x{:08X} ex=0x{:08X}",
            show_previous_visible,
            IsWindowVisible(widget_hwnd) != 0,
            GetWindowLongPtrW(widget_hwnd, GWL_STYLE) as u32,
            GetWindowLongPtrW(widget_hwnd, GWL_EXSTYLE) as u32
        ));
        boost_z_order(widget_hwnd)?;
        record_native_event(format!(
            "attach after-boost top={:#x} is_top={} detail=[{}]",
            GetTopWindow(taskbar_hwnd) as usize,
            GetTopWindow(taskbar_hwnd) == widget_hwnd,
            describe_window_detailed(widget_hwnd)
        ));
        if let Err(error) = ensure_low_level_mouse_hook(widget_hwnd) {
            record_native_event(format!("mouse-hook install failed: {error}"));
        }
        let descendant_guard_report = install_webview_descendant_guards(widget_hwnd);
        record_native_event(format!("guards attach {descendant_guard_report}"));

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

        let _ = (widget_width, taskbar_height, dpi);
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
    _logical_width: i32,
    _position: WidgetPosition,
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
    let physical_width = taskbar_width;
    let widget_x = 0;

    SetLastError(0);
    let set_pos_result = SetWindowPos(
        widget_hwnd,
        HWND_TOP,
        widget_x,
        0,
        physical_width,
        taskbar_height,
        SWP_NOACTIVATE | SWP_SHOWWINDOW | SWP_NOOWNERZORDER,
    );
    let set_pos_error = GetLastError();
    if set_pos_result == 0
    {
        return Err(format!("无法应用页面宽度，Win32 错误码：{set_pos_error}"));
    }
    boost_z_order(widget_hwnd)?;
    Ok(())
}

/// 将组件提升到当前父窗口的子窗口 Z-order 顶端，不激活窗口也不改变尺寸位置。
unsafe fn boost_z_order(widget_hwnd: HWND) -> Result<(), String> {
    if SetWindowPos(
        widget_hwnd,
        HWND_TOP,
        0,
        0,
        0,
        0,
        SWP_NOMOVE
            | SWP_NOSIZE
            | SWP_NOACTIVATE
            | SWP_SHOWWINDOW
            | SWP_NOOWNERZORDER,
    ) == 0
    {
        return Err(last_error("无法提升任务栏组件 Z-order"));
    }
    Ok(())
}


/// 保存前端上报的交互矩形。
/// main WebView 本身保持点透；WH_MOUSE_LL 只在这些区域内拦截鼠标按键，
/// 区域之外的点击继续交给 Windows 任务栏。
pub fn set_interactive_regions(
    window: &WebviewWindow,
    regions: &[InteractiveRegion],
    webview_scale_factor: f64,
) -> Result<(), String> {
    let native = window
        .hwnd()
        .map_err(|error| format!("无法获取组件窗口句柄：{error}"))?;
    let widget_hwnd = native.0 as HWND;
    if widget_hwnd.is_null() {
        return Err("任务栏组件窗口句柄为空".to_string());
    }

    unsafe {
        let mut window_rect: RECT = std::mem::zeroed();
        if GetWindowRect(widget_hwnd, &mut window_rect) == 0 {
            return Err(last_error("无法读取任务栏组件窗口区域"));
        }

        let scale = if webview_scale_factor.is_finite() && webview_scale_factor > 0.25 {
            webview_scale_factor
        } else {
            GetDpiForWindow(widget_hwnd).max(96) as f64 / 96.0
        };
        let mut physical_regions = Vec::with_capacity(regions.len());

        for region in regions {
            if !region.x.is_finite()
                || !region.y.is_finite()
                || !region.width.is_finite()
                || !region.height.is_finite()
                || region.width <= 0.0
                || region.height <= 0.0
            {
                continue;
            }

            let left = window_rect.left + (region.x * scale).floor() as i32;
            let top = window_rect.top + (region.y * scale).floor() as i32;
            let right = window_rect.left + ((region.x + region.width) * scale).ceil() as i32;
            let bottom = window_rect.top + ((region.y + region.height) * scale).ceil() as i32;

            let left = left.clamp(window_rect.left, window_rect.right);
            let top = top.clamp(window_rect.top, window_rect.bottom);
            let right = right.clamp(window_rect.left, window_rect.right);
            let bottom = bottom.clamp(window_rect.top, window_rect.bottom);
            if right <= left || bottom <= top {
                continue;
            }

            physical_regions.push(PhysicalInteractiveRegion {
                id: region.id.clone(),
                left,
                top,
                right,
                bottom,
            });
        }

        let storage = INTERACTIVE_REGIONS.get_or_init(|| Mutex::new(Vec::new()));
        let mut current = storage
            .lock()
            .map_err(|error| format!("无法锁定任务栏交互区域：{error}"))?;
        let changed = current.len() != physical_regions.len()
            || current.iter().zip(&physical_regions).any(|(a, b)| {
                a.id != b.id || a.left != b.left || a.top != b.top || a.right != b.right || a.bottom != b.bottom
            });
        *current = physical_regions;
        if INPUT_DEBUG_LOGS && changed {
            let ids = current.iter().map(|region| region.id.as_str()).collect::<Vec<_>>().join(",");
            record_native_event(format!("interactive-regions count={} scale={:.3} ids=[{}]", current.len(), scale, ids));
        }
    }

    Ok(())
}


/// 采样当前鼠标命中的原生 HWND，同时主动把组件重新提升到 Shell_TrayWnd 子窗口栈顶。
/// 仅在命中状态变化、Z-order/parent 异常或出现真实鼠标事件时返回日志，避免 hover 持续刷屏。
pub fn reinforce_and_probe(window: &WebviewWindow) -> Result<Option<String>, String> {
    let native = window
        .hwnd()
        .map_err(|error| format!("无法获取组件窗口句柄：{error}"))?;
    let widget_hwnd = native.0 as HWND;
    if widget_hwnd.is_null() {
        return Err("任务栏组件窗口句柄为空".to_string());
    }

    unsafe {
        let taskbar_hwnd = find_primary_taskbar()?;
        MOUSE_HOOK_WIDGET_HWND.store(widget_hwnd as usize, Ordering::Relaxed);

        let style = GetWindowLongPtrW(widget_hwnd, GWL_STYLE);
        let cleaned_style = (style as u32 & !WS_DISABLED) as isize;
        if cleaned_style != style {
            set_window_style(
                widget_hwnd,
                GWL_STYLE,
                cleaned_style,
                "诊断阶段清理 WS_DISABLED 失败",
            )?;
        }

        let ex_style = GetWindowLongPtrW(widget_hwnd, GWL_EXSTYLE);
        let cleaned_ex_style = (ex_style as u32 & !(WS_EX_TRANSPARENT | WS_EX_NOACTIVATE)) as isize;
        if cleaned_ex_style != ex_style {
            set_window_style(
                widget_hwnd,
                GWL_EXSTYLE,
                cleaned_ex_style,
                "诊断阶段清理窗口扩展样式失败",
            )?;
        }
        boost_z_order(widget_hwnd)?;
        let descendant_guard_report = install_webview_descendant_guards(widget_hwnd);

        let top_after = GetTopWindow(taskbar_hwnd);
        let parent_after = GetParent(widget_hwnd);
        let mut cursor = POINT { x: 0, y: 0 };
        if GetCursorPos(&mut cursor) == 0 {
            return Err(last_error("无法读取鼠标位置"));
        }

        let hit_hwnd = WindowFromPoint(cursor);
        let mut widget_rect: RECT = std::mem::zeroed();
        if GetWindowRect(widget_hwnd, &mut widget_rect) == 0 {
            return Err(last_error("无法读取组件屏幕矩形"));
        }
        let widget_width = widget_rect.right - widget_rect.left;
        let widget_height = widget_rect.bottom - widget_rect.top;
        let cursor_inside = cursor.x >= widget_rect.left
            && cursor.x < widget_rect.right
            && cursor.y >= widget_rect.top
            && cursor.y < widget_rect.bottom;

        let hit_summary = describe_window_compact(hit_hwnd);
        let hit_chain = describe_parent_chain_compact(hit_hwnd, 6);
        let z_order_abnormal = top_after != widget_hwnd;
        let parent_abnormal = parent_after != taskbar_hwnd;

        let native_events = NATIVE_EVENT_QUEUE
            .get_or_init(|| Mutex::new(VecDeque::with_capacity(32)))
            .lock()
            .map(|mut queue| queue.drain(..).collect::<Vec<_>>())
            .unwrap_or_default();
        let has_native_events = !native_events.is_empty();

        let signature = format!(
            "inside={cursor_inside}|hit={:#x}|top={:#x}|parent={:#x}|guards={descendant_guard_report}",
            hit_hwnd as usize,
            top_after as usize,
            parent_after as usize,
        );
        let state = PROBE_STATE.get_or_init(|| Mutex::new(ProbeState::default()));
        let mut state = state.lock().map_err(|error| format!("诊断状态锁失败：{error}"))?;
        let changed = state.last_signature != signature;
        let should_emit = has_native_events
            || z_order_abnormal
            || parent_abnormal
            || (cursor_inside && changed);
        if !should_emit {
            return Ok(None);
        }
        state.last_signature = signature;

        let mut flags = Vec::new();
        if z_order_abnormal {
            flags.push("z-order");
        }
        if parent_abnormal {
            flags.push("parent");
        }
        let flags_text = if flags.is_empty() {
            String::new()
        } else {
            format!(" abnormal=[{}]", flags.join(","))
        };

        if has_native_events {
            return Ok(Some(format!(
                "输入事件 [{}]",
                native_events.join(" | ")
            )));
        }

        Ok(Some(format!(
            "输入诊断 inside={} size={}x{} hit={} chain=[{}] {}{}",
            cursor_inside,
            widget_width,
            widget_height,
            hit_summary,
            hit_chain,
            descendant_guard_report,
            flags_text,
        )))
    }
}

unsafe fn window_class_name(hwnd: HWND) -> String {
    if hwnd.is_null() || IsWindow(hwnd) == 0 {
        return "<invalid>".to_string();
    }
    let mut class_buffer = [0u16; 256];
    let class_len = GetClassNameW(hwnd, class_buffer.as_mut_ptr(), class_buffer.len() as i32);
    if class_len > 0 {
        String::from_utf16_lossy(&class_buffer[..class_len as usize])
    } else {
        "<unknown>".to_string()
    }
}

unsafe fn describe_window_compact(hwnd: HWND) -> String {
    if hwnd.is_null() || IsWindow(hwnd) == 0 {
        return format!("<invalid>@{:#x}", hwnd as usize);
    }

    let mut process_id = 0u32;
    let thread_id = GetWindowThreadProcessId(hwnd, &mut process_id);
    format!(
        "{}@{:#x}({}/{})",
        window_class_name(hwnd),
        hwnd as usize,
        process_id,
        thread_id
    )
}

unsafe fn describe_parent_chain_compact(mut hwnd: HWND, limit: usize) -> String {
    let mut parts = Vec::new();
    for _ in 0..limit {
        if hwnd.is_null() || IsWindow(hwnd) == 0 {
            break;
        }
        parts.push(window_class_name(hwnd));
        let parent = GetParent(hwnd);
        if parent == hwnd {
            break;
        }
        hwnd = parent;
    }
    parts.join(" > ")
}





/// 返回主任务栏右侧通知区域占用的逻辑宽度。
/// 靠右布局时前端用它作为右侧内边距，避免插件覆盖托盘图标/时钟区域。
pub fn taskbar_right_reserved_logical_width() -> Result<f64, String> {
    let taskbar = find_primary_taskbar()?;
    let tray_class: Vec<u16> = "TrayNotifyWnd\0".encode_utf16().collect();

    unsafe {
        let tray = FindWindowExW(taskbar, ptr::null_mut(), tray_class.as_ptr(), ptr::null());
        if tray.is_null() {
            return Ok(0.0);
        }

        let mut taskbar_rect: RECT = std::mem::zeroed();
        let mut tray_rect: RECT = std::mem::zeroed();
        if GetWindowRect(taskbar, &mut taskbar_rect) == 0 || GetWindowRect(tray, &mut tray_rect) == 0 {
            return Err(last_error("无法读取任务栏托盘区域尺寸"));
        }

        let reserved_physical = (taskbar_rect.right - tray_rect.left).max(0);
        let dpi = GetDpiForWindow(taskbar).max(96);
        Ok(f64::from(physical_to_logical_width(reserved_physical, dpi)))
    }
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
